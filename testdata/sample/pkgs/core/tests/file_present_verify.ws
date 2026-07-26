use value
use fs

// Custom assertions for the file_present_converges test; runs inside
// the container after the apply runs. `facts` carries the gather-test
// results (the "os" map here).
fn verify(facts: Value) -> Result[bool, string] {
    Ok(fs::read("/var/tmp/weave-sample.txt")? == "hello")
}
