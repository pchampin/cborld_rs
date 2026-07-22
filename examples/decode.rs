fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdin = std::io::stdin();
    let json2 = cborld::cborld_to_jsonld(&mut stdin, [])?;
    print!("{}", serde_json::to_string_pretty(&json2)?);
    Ok(())
}
