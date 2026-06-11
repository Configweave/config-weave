use value
use sys

fn gather(params: Value) -> Value {
    Value::Map(#{
        "family": Value::String(sys::family()),
        "name": Value::String(sys::os_name()),
        "version": Value::String(sys::os_version()),
        "arch": Value::String(sys::arch()),
        "cpus": Value::Int(sys::cpu_count())
    })
}
