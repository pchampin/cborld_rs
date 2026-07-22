use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../test_factory/");

    let out_dir = env::var_os("OUT_DIR").unwrap();
    let names = cborld_test_factory::TestFactory::new(&out_dir).make()?;

    let mut tests_in = fs::File::create(Path::new(&out_dir).join("tests.in"))?;

    for path in &names {
        let name = path.file_name().unwrap();
        writeln!(&mut tests_in, "#[test_case({path:?}; {name:?})]")?;
    }
    writeln!(
        &mut tests_in,
        r#"fn decode(name: &str) -> TestResult {{ run_decode(name) }}"#
    )?;

    for path in &names {
        let name = path.file_name().unwrap();
        writeln!(&mut tests_in, "#[test_case({path:?}; {name:?})]")?;
    }
    writeln!(
        &mut tests_in,
        r#"fn encode(name: &str) -> TestResult {{ run_encode(name) }}"#
    )?;

    Ok(())
}
