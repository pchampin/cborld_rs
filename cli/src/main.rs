use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = std::env::args().nth(1).unwrap();
    match subcommand.as_str() {
        "encode" => {
            let reg: u64 = std::env::args().nth(2).unwrap().parse()?;
            let mut stdin = std::io::stdin();
            let jsonld: serde_json::Value = serde_json::from_reader(&mut stdin)?;

            let registry_entry = cborld::get_registry_entry(reg).unwrap();
            let type_table = &registry_entry.type_tables;
            let mut bytes: Vec<u8> = vec![];
            cborld::jsonld_to_cborld(type_table.clone(), reg, jsonld, &mut bytes, [])?;
            std::io::stdout().write_all(&bytes)?;
        }
        "decode" => {
            let mut stdin = std::io::stdin();
            let json2 = cborld::cborld_to_jsonld(&mut stdin, [])?;
            print!("{}", serde_json::to_string_pretty(&json2)?);
        }
        "roundtrip" => {
            let reg: u64 = std::env::args().nth(2).unwrap().parse()?;
            let mut stdin = std::io::stdin();
            let jsonld: serde_json::Value = serde_json::from_reader(&mut stdin)?;

            let registry_entry = cborld::get_registry_entry(reg).unwrap();
            let type_table = &registry_entry.type_tables;
            let mut bytes: Vec<u8> = vec![];
            cborld::jsonld_to_cborld(type_table.clone(), reg, jsonld, &mut bytes, [])?;

            let json2 = cborld::cborld_to_jsonld(&bytes[..], [])?;
            print!("{}", serde_json::to_string_pretty(&json2)?);
        }
        _ => panic!("unknown subcommand {subcommand:?}"),
    }
    Ok(())
}
