# Java agent that sets IO priority for threads

At the moment it will not build as it requireі patched `rust-jvmti` library.`

## Arguments

- `thread_name` is a regular expression that should match a thread name
- `prio` is a priorty class. It may be either `idle` or `best_effort(level)` where `level` is a number from `0` to `7` where `0` is the highest level and `7` is the lowest one

## How to run

``` sh
java -agentpath:libjava_ionice.so="thread_name=reader_1,prio=idle;thread_name=reader_2,prio=best_effort(7)" Main
```
