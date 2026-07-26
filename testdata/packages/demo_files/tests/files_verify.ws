use value
use fs

// Runs inside the container after the three engine runs: the converged
// state must actually be on disk.
fn verify(facts: Value) -> Result[bool, string] {
    if !fs::is_dir("/opt/demo") {
        return Ok(false)
    }
    Ok(fs::read("/opt/demo/app.conf")? == "greeting=hello\n")
}
