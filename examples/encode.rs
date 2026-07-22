use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reg: u64 = std::env::args().nth(1).unwrap().parse()?;
    let mut stdin = std::io::stdin();
    let jsonld: serde_json::Value = serde_json::from_reader(&mut stdin)?;

    let registry_entry = cborld::get_registry_entry(reg).unwrap();
    let type_table = &registry_entry.type_tables;
    let mut bytes: Vec<u8> = vec![];
    cborld::jsonld_to_cborld(type_table.clone(), reg, jsonld, &mut bytes, [])?;
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}
