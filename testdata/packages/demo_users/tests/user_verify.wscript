use value
use fs

fn verify(facts: Value) -> Result[bool, string] {
    let passwd = fs::read("/etc/passwd")?
    for line in passwd.split("\n") {
        if line.starts_with("weavedemo:") {
            return Ok(true)
        }
    }
    Ok(false)
}
