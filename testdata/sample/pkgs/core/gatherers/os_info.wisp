use value

fn gather(params: Value) -> Value {
    // M3 replaces this hand-rolled map with the `sys` module.
    let family = "linux"
    Value::Map(#{
        "family": Value::String(family),
        "arch": Value::String("x86_64")
    })
}
