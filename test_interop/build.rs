use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../test_factory/");

    let out_dir = env::var_os("OUT_DIR").unwrap();
    cborld_test_factory::TestFactory::new(&out_dir).make()?;

    Ok(())
}
