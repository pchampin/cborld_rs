use anyhow::{Context, Result};
use serde_cbor::Value as CborValue;
use serde_json::Value as JsonValue;
use std::{
    env::set_current_dir,
    fs::read_to_string,
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
};

fn main() -> Result<()> {
    let mut config: Config =
        toml::from_str(&read_to_string("config.toml").context("opening config.toml")?)
            .context("can not parse config.toml")?;
    if let Some(filter) = std::env::args().nth(1) {
        config.filter = Some(filter);
    };
    let test_dir = Path::new(env!("OUT_DIR")).join("tests");
    set_current_dir(&test_dir)
        .with_context(|| format!("setting `{}` as current dir", test_dir.to_string_lossy()))?;
    let https = ChildGuard(
        Command::new(&config.http_server.cmd)
            .stdout(Stdio::null())
            .spawn()
            .with_context(|| format!("spawning HTTP server {}", &config.http_server.cmd))?,
    );
    do_tests(config)?;
    drop(https);
    Ok(())
}

fn do_tests(config: Config) -> Result<()> {
    for f in std::path::Path::new(".")
        .read_dir()
        .context("reading tests directory")?
    {
        let f = f.context("browsing tests dir")?.path();
        let f_txt = f.to_string_lossy();
        if !f_txt.ends_with(".jsonld") {
            continue;
        }
        if let Some(f) = config.filter.as_ref()
            && !f_txt.contains(f)
        {
            continue;
        }
        let jsonld = read_to_string(&f).with_context(|| format!("reading {}", f_txt))?;
        for reg_id in [0, 1] {
            let cborld_ref_bytes = config.encode(0, &jsonld, reg_id)?;
            let cborld_ref: CborValue = serde_cbor::from_slice(&cborld_ref_bytes)
                .with_context(|| format!("decoding encoding of {} with tool 0", f_txt))?;
            let ref_name = &config.tools[0].name;
            for i in 1..config.tools.len() {
                let tool_name = &config.tools[i].name;
                if !test(
                    format!("{f_txt}\tEncode\t{reg_id}\t{ref_name}={tool_name}"),
                    || {
                        let cborld_bytes = config.encode(i, &jsonld, reg_id)?;
                        let cborld: CborValue = serde_cbor::from_slice(&cborld_bytes)
                            .with_context(|| {
                                format!("decoding encoding of {} with tool {i}", f_txt)
                            })?;
                        (cborld_ref != cborld)
                            .then(|| Ok(format!("EXP: {cborld_ref:?}\nGOT: {cborld:?}")))
                            .or_else(|| {
                                (cborld_ref_bytes.len() != cborld_bytes.len()).then(|| {
                                    Ok(format!(
                                        "EXP: {} bytes\nGOT: {} bytes",
                                        cborld_ref_bytes.len(),
                                        cborld_bytes.len()
                                    ))
                                })
                            })
                            .transpose()
                    },
                ) && config.stop_on_err
                {
                    return Ok(());
                }
            }
            for i in 0..config.tools.len() {
                let tool1_name = &config.tools[i].name;
                for j in 0..config.tools.len() {
                    let tool2_name = &config.tools[j].name;
                    if !test(
                        format!("{f_txt}\tRoundtrip\t{reg_id}\t{tool1_name}>{tool2_name}"),
                        || {
                            let cborld = config.encode(i, &jsonld, reg_id)?;
                            let jsonld2 = config.decode(j, &cborld)?;
                            let jsonld1: JsonValue = serde_json::from_str(&jsonld)
                                .with_context(|| format!("parsing {} in JSON", f_txt))?;
                            let jsonld2: JsonValue =
                                serde_json::from_str(&jsonld2).with_context(|| {
                                    format!("parsing in JSON result of tool {j} for {}", f_txt)
                                })?;
                            (jsonld1 != jsonld2)
                                .then(|| Ok(format!("EXP: {jsonld1:?}\nGOT: {jsonld2:?}")))
                                .transpose()
                        },
                    ) && config.stop_on_err
                    {
                        return Ok(());
                    }
                }
            }
        }
    }
    Ok(())
}

fn test<F: FnOnce() -> Result<Option<String>>>(name: String, f: F) -> bool {
    print!("======== ");
    match f() {
        Ok(None) => {
            println!("OK   {name}");
            true
        }
        Ok(Some(diag)) => {
            println!("FAIL {name}\n     {diag}");
            false
        }
        Err(err) => {
            println!("ERR  {name}\n     {err}");
            for cause in err.chain().skip(1) {
                println!("     {cause}");
            }
            false
        }
    }
}

#[derive(serde::Deserialize)]
struct Config {
    stop_on_err: bool,
    tools: Vec<Tool>,
    http_server: HttpServer,
    filter: Option<String>,
}

impl Config {
    fn encode(&self, tool: usize, jsonld: &str, reg_id: usize) -> Result<Vec<u8>> {
        let tool = &self.tools[tool];
        let mut child = Command::new(&tool.cmd)
            .args(["encode", &format!("{reg_id}")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning {}", &tool.cmd))?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(jsonld.as_bytes())
            .with_context(|| format!("writing JSON-LD to stdin of {}", &tool.cmd))?;
        Ok(child
            .wait_with_output()
            .with_context(|| format!("waiting for completion of {} encode {reg_id}", &tool.cmd))?
            .stdout)
    }

    fn decode(&self, tool: usize, cborld: &[u8]) -> Result<String> {
        let tool = &self.tools[tool];
        let mut child = Command::new(&tool.cmd)
            .args(["decode"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning {}", &tool.cmd))?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(cborld)
            .with_context(|| format!("writing JSON-LD to stdin of {}", &tool.cmd))?;
        String::from_utf8(
            child
                .wait_with_output()
                .with_context(|| format!("waiting for completion of {} decode", &tool.cmd))?
                .stdout,
        )
        .with_context(|| format!("converting to UTF-8 result of {} decode", &tool.cmd))
    }
}

#[derive(serde::Deserialize)]
struct Tool {
    name: String,
    cmd: String,
}

#[derive(serde::Deserialize)]
struct HttpServer {
    cmd: String,
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.0.kill().unwrap_or(())
    }
}
