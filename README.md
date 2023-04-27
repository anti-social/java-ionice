# Java agent that sets IO priority for threads

At the moment it will not build as it is required patched `rust-jvmti` library.`

## Arguments

- `thread_name` is a regular expression that should match thread name
- `prio` is a priorty class. It may be either `idle` or `best_priority(level)` where `level` is a number from 1 to 7

## How to run

``` sh
java -agentpath:libjava_ionice.so=thread_name=reader_1,prio=idle;thread_name=reader_2,prio=best_effort(7) Main
```
