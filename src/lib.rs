use std::ffi::{c_char, CStr};
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicI64, Ordering};

use dlopen::symbor::{Library, SymBorApi, Ref};
#[macro_use]
extern crate dlopen_derive;

use jvmti::options::Options;
use jvmti::util::stringify;
use jvmti::{
    native::{
        JavaVMPtr, VoidPtr, MutString,
    },
    agent::Agent,
    thread::Thread,
    context::static_context,
    environment::{
        jni::JNI,
        Environment,
    },
};

use lazy_static::lazy_static;

use regex::Regex;

lazy_static! {
    static ref THREADS_PRIO: Arc<RwLock<Vec<ThreadPrio>>> = Arc::new(RwLock::new(vec!()));
}

static OSTHREAD_FIELD_OFFSET: AtomicI64 = AtomicI64::new(-1);
static OS_THREAD_ID_FIELD_OFFSET: AtomicI64 = AtomicI64::new(-1);

#[derive(SymBorApi)]
struct VMSymbols<'a> {
    #[dlopen_name="gHotSpotVMStructs"]
    structs: Ref<'a, usize>,
    #[dlopen_name="gHotSpotVMStructEntryArrayStride"]
    stride: Ref<'a, usize>,
    #[dlopen_name="gHotSpotVMStructEntryTypeNameOffset"]
    type_offset: Ref<'a, usize>,
    #[dlopen_name="gHotSpotVMStructEntryFieldNameOffset"]
    field_offset: Ref<'a, usize>,
    #[dlopen_name="gHotSpotVMStructEntryOffsetOffset"]
    offset_offset: Ref<'a, usize>,
}

#[derive(Debug)]
struct ThreadPrio {
    pub thread_name: Regex,
    pub prio_class: ioprio::Class,
}

#[no_mangle]
#[allow(non_snake_case, unused_variables)]
pub extern "C" fn Agent_OnLoad(
    vm: JavaVMPtr,
    options: MutString,
    reserved: VoidPtr,
) {
    println!("Hello from jvmti agent");
    let mut threads_prio = vec!();
    for thread_options in stringify(options).split(';') {
        let thread_options = Options::parse(thread_options.to_string());
        let thread_name = match thread_options.custom_args.get("thread_name") {
            Some(name) => {
                if let Ok(name_re) = Regex::new(&name) {
                    name_re
                } else {
                    continue
                }
            }
            None => continue,
        };
        let prio_class = match thread_options.custom_args.get("prio") {
            Some(prio) => {
                if prio == "idle" {
                    ioprio::Class::Idle
                } else {
                    match prio.trim_end_matches(')').strip_prefix("best_effort(") {
                        Some(level) => {
                            if let Ok(level) = level.parse() {
                                if let Some(level) = ioprio::BePriorityLevel::from_level(level) {
                                    ioprio::Class::BestEffort(level)
                                } else {
                                    continue
                                }
                            } else {
                                continue
                            }
                        }
                        None => continue,
                    }
                }
            }
            None => continue,
        };
        threads_prio.push(
            ThreadPrio { thread_name: thread_name.clone(), prio_class }
        );
    }
    match THREADS_PRIO.clone().write() {
        Ok(mut prios) => (*prios).extend(threads_prio),
        Err(_) => {}
    }

    // for thread_option in thread_options.split(',') {
    //     // thread_option.sp
    // }

    match Library::open("libjvm.so") {
        Ok(libjvm) => {
            println!("libjvm.so successfully opened");
            let vm_symbols = unsafe{ VMSymbols::load(&libjvm) }
                .expect("Could not load symbols");
            println!("VM structs address: 0x{:x}", *vm_symbols.structs);
            println!("VM entry stride: 0x{:x}", *vm_symbols.stride);
            println!("VM type offset: 0x{:x}", *vm_symbols.type_offset);
            println!("VM field offset: 0x{:x}", *vm_symbols.field_offset);
            println!("VM offset offset: 0x{:x}", *vm_symbols.offset_offset);

            let mut cur_entry_addr = *vm_symbols.structs;
            loop {
                let entry_type_ptr = unsafe {
                    *((cur_entry_addr + *vm_symbols.type_offset) as *const *const c_char)
                };
                let field_name_ptr = unsafe {
                    *((cur_entry_addr + *vm_symbols.field_offset) as *const *const c_char)
                };
                if entry_type_ptr.is_null() || field_name_ptr.is_null() {
                    break;
                }
                let entry_type = unsafe { CStr::from_ptr(entry_type_ptr) }.to_string_lossy();
                let field_name = unsafe { CStr::from_ptr(field_name_ptr) }.to_string_lossy();
                match (entry_type.as_ref(), field_name.as_ref()) {
                    ("JavaThread", "_osthread") => {
                        let osthread_field_offset_addr = cur_entry_addr + *vm_symbols.offset_offset;
                        let osthread_field_offset = unsafe { *(osthread_field_offset_addr as *const i32) };
                        println!("Found JavaThread _osthread offset: 0x{:x}", osthread_field_offset);
                        OSTHREAD_FIELD_OFFSET.store(osthread_field_offset as i64, Ordering::SeqCst);
                        println!("Store OSTHREAD_FIELD_OFFSET={}", OSTHREAD_FIELD_OFFSET.load(Ordering::SeqCst))
                    }
                    ("OSThread", "_thread_id") => {
                        let os_thread_id_field_offset_addr = cur_entry_addr + *vm_symbols.offset_offset;
                        let os_thread_id_field_offset = unsafe { *(os_thread_id_field_offset_addr as *const i32) };
                        println!("Found OSThread _thread_id offset: 0x{:x}", os_thread_id_field_offset);
                        OS_THREAD_ID_FIELD_OFFSET.store(os_thread_id_field_offset as i64, Ordering::SeqCst);
                    }
                    (_, _) => {}
                }
                cur_entry_addr += *vm_symbols.stride;
            }
        },
        Err(e) => println!("Missing libjvm"),
    }

    let mut agent = Agent::new(vm);
    agent.on_thread_start(Some(on_thread_start));

    agent.update();
}

pub fn on_thread_start(env: Environment, thread: Thread) {
    let java_thread = thread.id.native_id;
    println!(
        "New thread started: id={:?}, name={}",
        java_thread,
        thread.name
    );

    let mut thread_prio_class = None;
    match THREADS_PRIO.clone().read() {
        Ok(threads_prio) => {
            for thread_prio in threads_prio.iter() {
                if thread_prio.thread_name.is_match(&thread.name) {
                    thread_prio_class = Some(thread_prio.prio_class);
                }
            }
        }
        Err(_) => {}
    }

    if let Some(thread_prio_class) = thread_prio_class {
        let java_thread = thread.id.native_id;
        let java_thread_class = env.get_object_class(&java_thread);
        // let tid_field_id =  env.get_field_id(&java_thread_class, "tid", "J");
        let eetop_field_id =  env.get_field_id(&java_thread_class, "eetop", "J");
        let eetop_addr = env.get_long_field(&java_thread, eetop_field_id);
        println!("Thread's eetop address: 0x{:x}", eetop_addr);
        let osthread_field_offset = OSTHREAD_FIELD_OFFSET.load(Ordering::SeqCst);
        if osthread_field_offset < 0 {
            return;
        }
        let os_thread_id_field_offset = OS_THREAD_ID_FIELD_OFFSET.load(Ordering::SeqCst) as isize;
        if os_thread_id_field_offset < 0 {
            return;
        }

        let os_thread_addr = unsafe {
            *((eetop_addr + osthread_field_offset) as *const isize)
        };
        let os_thread_id = unsafe {
            *((os_thread_addr + os_thread_id_field_offset) as *const i32)
        };
        println!("OS thread id is: {}", os_thread_id);

        let set_prio_result = ioprio::set_priority(
            ioprio::Target::Process(ioprio::Pid::from_raw(os_thread_id)),
            ioprio::Priority::new(thread_prio_class)
        );
        match set_prio_result {
            Ok(()) => println!("IO priority {:?} successfully set for a thread {}", thread_prio_class, thread.name),
            Err(e) => println!("Error when setting IO priority for a thread {}: {}", thread.name, e),
        }
    }

    static_context().thread_start(&thread.id);
}
