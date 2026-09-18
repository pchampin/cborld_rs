use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use gc_value::GcValue;
use serde_json::{Value as JsonValue, json};

pub struct TestFactory {
    tests: PathBuf,
    c1_context: GcValue,
    encode: HashMap<&'static str, usize>,
}

impl TestFactory {
    pub fn new(out_dir: impl Into<OsString>) -> Self {
        let tests = Path::new(&out_dir.into()).join("tests");
        let c1_context: GcValue = serde_json::from_value(json!({
            "p": "x:p",
            "iri": {
              "@id": "x:iri",
              "@type": "@id"
            },
            "lst": {
              "@id": "x:lst",
              "@container": "@list"
            },
            "iris": {
              "@id": "x:iris",
              "@type": "@id",
              "@container": "@list"
            },
            "lbl": {
              "@id": "x:lbl",
              "@container": "@language"
            },
            "ids": {
              "@id": "x:ids",
              "@container": "@id"
            },
            "xsdate": {
              "@id": "x:xdate",
              "@type": "http://www.w3.org/2001/XMLSchema#date"
            }
        }))
        .unwrap();
        let encode = HashMap::from([
            ("ids", 100_usize),
            ("iri", 102),
            ("iris", 104),
            ("lbl", 106),
            ("lst", 108),
            ("p", 110),
            ("xsdate", 112),
        ]);

        Self {
            tests,
            c1_context,
            encode,
        }
    }

    pub fn make(&self) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
        if fs::exists(&self.tests)? {
            fs::remove_dir_all(&self.tests)?;
        }
        fs::create_dir(&self.tests)?;
        let ctx = self.tests.join("ctx");
        fs::create_dir(&ctx)?;

        serde_json::to_writer_pretty(
            fs::File::create_new(ctx.join("c1.jsonld"))?,
            &GcValue::collect_map([("@context", self.c1_context.clone())]),
        )?;

        #[expect(clippy::approx_constant)]
        let unit_test_data = [
            ("none_int_small_positive", "p", json!(1), GcValue::from(1)),
            ("none_int_small_negative", "p", json!(-1), GcValue::from(-1)),
            (
                "none_int_big_positive",
                "p",
                json!(9_007_199_254_740_991_u64),
                GcValue::from(9_007_199_254_740_991_u64),
            ),
            (
                "none_int_big_negative",
                "p",
                json!(-9_007_199_254_740_991_i64),
                GcValue::from(-9_007_199_254_740_991_i64),
            ),
            (
                "none_plural",
                "p",
                json!(["o1", 42, 3.14, true]),
                GcValue::new_array(["o1".into(), 42.into(), 3.14.into(), true.into()]),
            ),
            (
                "none_plural_list",
                "lst",
                json!(["o1", 42, 3.14, true]),
                GcValue::new_array(["o1".into(), 42.into(), 3.14.into(), true.into()]),
            ),
            (
                "url_data_b64",
                "iri",
                json!("data:application/octet-stream;base64,q83vQg=="),
                GcValue::new_array([
                    4.into(),
                    "application/octet-stream".into(),
                    b"\xab\xcd\xef\x42".into(),
                ]),
            ),
            (
                "url_data_b64_nocanon",
                "iri",
                json!("data:application/octet-stream;base64,QR=="),
                GcValue::new_array([4.into(), "application/octet-stream;base64,QR==".into()]),
            ),
            (
                "url_data_txt",
                "iri",
                json!("data:text/plain,hello%20world"),
                GcValue::new_array([4.into(), "text/plain,hello%20world".into()]),
            ),
            (
                "url_did_key_b58",
                "iri",
                json!("did:key:z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv"),
                GcValue::new_array([1025.into(), b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into()]),
            ),
            (
                "url_did_key_b58_b58",
                "iri",
                json!("did:key:z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv#z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv"),
                GcValue::new_array([
                    1025.into(),
                    b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into(),
                    b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into(),
                ]),
            ),
            (
                "url_did_key_b58_txt",
                "iri",
                json!("did:key:z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv#something"),
                GcValue::new_array([
                    1025.into(),
                    b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into(),
                    "something".into(),
                ]),
            ),
            (
                "url_did_key_b64",
                "iri",
                json!("did:key:uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd"),
                GcValue::new_array([1025.into(), "uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd".into()]),
            ),
            (
                "url_did_key_b64_b58",
                "iri",
                json!("did:key:uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd#z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv"),
                GcValue::new_array([1025.into(), "uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd".into(),
                    b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into(),
                ]),
            ),
            (
                "url_did_key_b64_txt",
                "iri",
                json!("did:key:uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd#something"),
                GcValue::new_array([1025.into(), "uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd".into(), "something".into()]),
            ),
            (
                "url_did_v1_nym_b58",
                "iri",
                json!("did:v1:nym:z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv"),
                GcValue::new_array([1024.into(), b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into()]),
            ),
            (
                "url_did_v1_nym_b58_b58",
                "iri",
                json!("did:v1:nym:z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv#z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv"),
                GcValue::new_array([
                    1024.into(),
                    b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into(),
                    b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into(),
                ]),
            ),
            (
                "url_did_v1_nym_b58_txt",
                "iri",
                json!("did:v1:nym:z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv#something"),
                GcValue::new_array([
                    1024.into(),
                    b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into(),
                    "something".into(),
                ]),
            ),
            (
                "url_did_v1_nym_b64",
                "iri",
                json!("did:v1:nym:uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd"),
                GcValue::new_array([1024.into(), "uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd".into()]),
            ),
            (
                "url_did_v1_nym_b64_b58",
                "iri",
                json!("did:v1:nym:uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd#z6Mkw5LtCiW1Dn7dopLTqtcczWoBTQiBQLP6FrZeRYuvtAWv"),
                GcValue::new_array([1024.into(), "uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd".into(),
                    b"\xed\x01\xf6\xf9\x5e\xd5\x20\x4f\xcd\xb0\x5e\x22\x28\x76\x3f\xe5\xdb\x24\x21\xaf\x32\xe1\x3a\xd8\xf4\x92\x78\x18\x38\x92\xe6\xdc\x12\x93".into(),
                ]),
            ),
            (
                "url_did_v1_nym_b64_txt",
                "iri",
                json!("did:v1:nym:uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd#something"),
                GcValue::new_array([1024.into(), "uPcLIbKr4NG5mQWidS6N6fOlBpKtSp1jIzF9AVn7bHe9lbd".into(), "something".into()]),
            ),
            (
                "url_http",
                "iri",
                json!("http://example.org/o"),
                GcValue::new_array([1.into(), "example.org/o".into()]),
            ),
            (
                "url_https",
                "iri",
                json!("https://example.org/o"),
                GcValue::new_array([2.into(), "example.org/o".into()]),
            ),
            (
                "url_plural",
                "iri",
                json!(["x:o1", "http://example.org/o2"]),
                GcValue::new_array([
                    "x:o1".into(),
                    GcValue::new_array([1.into(), "example.org/o2".into()]),
                ]),
            ),
            (
                "url_plural_at_list",
                "iris",
                json!(["x:o1", "http://example.org/o2"]),
                GcValue::new_array([
                    "x:o1".into(),
                    GcValue::new_array([1.into(), "example.org/o2".into()]),
                ]),
            ),
            ("url_unknown", "iri", json!("x:o"), "x:o".into()),
            (
                "url_urn_uuid",
                "iri",
                json!("urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6"),
                GcValue::new_array([
                    3.into(),
                    b"\xf8\x1d\x4f\xae\x7d\xec\x11\xd0\xa7\x65\x00\xa0\xc9\x1e\x6b\xf6".into(),
                ]),
            ),
            (
                "xsd_date",
                "xsdate",
                json!("2026-09-18"),
                1789689600.into(),
            ),
            (
                "xsd_date_big",
                "xsdate",
                json!("10000-02-03"),
                253405152000_i64.into(),
            ),
            (
                "xsd_date_neg",
                "xsdate",
                json!("-2026-09-18"),
                (-126079113600_i64).into(),
            ),
            (
                "xsd_date_with_timezone",
                "xsdate",
                json!("2026-09-18Z"),
                "2026-09-18Z".into(),
            ),
            (
                "xsd_date_invalid",
                "xsdate",
                json!("foobar"),
                "foobar".into(),
            ),
            (
                "xsd_date_plural",
                "xsdate",
                json!(["2026-09-18", "1955-06-08", "foobar"]),
                GcValue::new_array([
                1789689600.into(),
                (-459734400).into(),
                "foobar".into(),
                ]),
            ),
        ];

        let mut paths = Vec::with_capacity(unit_test_data.len() * 3);
        paths.push(self.make_empty()?);

        for (name, key, json, cbor) in &unit_test_data {
            if *key != "xsdate" {
                paths.push(self.make_expanded(name, key, json, cbor)?);
            }
            paths.push(self.make_inline(name, key, json, cbor)?);
            paths.push(self.make_compact(name, key, json, cbor)?);
        }

        Ok(paths)
    }

    fn make_expanded(
        &self,
        name: &str,
        key: &str,
        json: &JsonValue,
        cbor: &GcValue,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let name = format!("exp_{name}");

        let (key, json, cbor) = prepare_expanded(key, json, cbor);

        let jsonld = fs::File::create_new(self.tests.join(format!("{name}.jsonld")))?;
        serde_json::to_writer_pretty(jsonld, &GcValue::collect_map([(key, json.clone())]))?;

        let mut cborld = fs::File::create_new(self.tests.join(format!("{name}.cborld")))?;
        cborld.write_all(b"\xD9\xCB\x1D\x82\x01")?;
        serde_cbor::to_writer(cborld, &GcValue::collect_map([(key, cbor.clone())]))?;

        Ok(self.tests.join(name))
    }

    fn make_empty(&self) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let mut jsonld = fs::File::create_new(self.tests.join("empty.jsonld"))?;
        jsonld.write_all(b"{}\n")?;

        let mut cborld = fs::File::create_new(self.tests.join("empty.cborld"))?;
        cborld.write_all(b"\xD9\xCB\x1D\x82\x01\xa0")?;

        Ok(self.tests.join("empty"))
    }

    fn make_inline(
        &self,
        name: &str,
        key: &str,
        json: &JsonValue,
        cbor: &GcValue,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let name = format!("inl_{name}");

        let jsonld = fs::File::create_new(self.tests.join(format!("{name}.jsonld")))?;
        serde_json::to_writer_pretty(
            jsonld,
            &GcValue::collect_map([
                ("@context", self.c1_context.clone()),
                (key, serde_json::from_value(json.clone())?),
            ]),
        )?;

        let offset = if json.is_array() { 1 } else { 0 };
        let key2 = *self.encode.get(key).unwrap() + offset;
        let mut cborld = fs::File::create_new(self.tests.join(format!("{name}.cborld")))?;
        cborld.write_all(b"\xD9\xCB\x1D\x82\x01")?;
        serde_cbor::to_writer(
            cborld,
            &GcValue::collect_map([(0, self.c1_context.clone()), (key2, cbor.clone())]),
        )?;

        Ok(self.tests.join(name))
    }

    fn make_compact(
        &self,
        name: &str,
        key: &str,
        json: &JsonValue,
        cbor: &GcValue,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let name = format!("com_{name}");

        let jsonld = fs::File::create_new(self.tests.join(format!("{name}.jsonld")))?;
        serde_json::to_writer_pretty(
            jsonld,
            &GcValue::collect_map([
                (
                    "@context",
                    GcValue::from("http://localhost:8000/ctx/c1.jsonld"),
                ),
                (key, serde_json::from_value(json.clone())?),
            ]),
        )?;

        let offset = if json.is_array() { 1 } else { 0 };
        let key2 = *self.encode.get(key).unwrap() + offset;
        let mut cborld = fs::File::create_new(self.tests.join(format!("{name}.cborld")))?;
        cborld.write_all(b"\xD9\xCB\x1D\x82\x01")?;
        serde_cbor::to_writer(
            cborld,
            &GcValue::collect_map([
                (0, GcValue::from("http://localhost:8000/ctx/c1.jsonld")),
                (key2, cbor.clone()),
            ]),
        )?;

        Ok(self.tests.join(name))
    }
}

fn prepare_expanded<'s>(
    key: &'s str,
    json: &JsonValue,
    cbor: &GcValue,
) -> (&'s str, GcValue, GcValue) {
    match key {
        "iri" => {
            let json = match json {
                JsonValue::Array(v) => {
                    GcValue::collect_array(v.iter().map(|j| prepare_expanded_single_json(key, j)))
                }
                single => prepare_expanded_single_json(key, single),
            };
            let cbor = if json.is_array() {
                GcValue::collect_array(
                    cbor.as_array()
                        .unwrap()
                        .iter()
                        .map(|c| prepare_expanded_single_cbor(key, c)),
                )
            } else {
                prepare_expanded_single_cbor(key, cbor)
            };
            ("x:iri", json, cbor)
        }
        "iris" => {
            let json = match json {
                JsonValue::Array(v) => GcValue::collect_map([(
                    "@list",
                    GcValue::collect_array(
                        v.iter().map(|j| prepare_expanded_single_json("iri", j)),
                    ),
                )]),
                _ => unreachable!("iris always expects array"),
            };
            let cbor = GcValue::collect_map([(
                21, // "@list" plural
                GcValue::collect_array(
                    cbor.as_array()
                        .unwrap()
                        .iter()
                        .map(|c| prepare_expanded_single_cbor("iri", c)),
                ),
            )]);
            ("x:iris", json, cbor)
        }
        "lst" => {
            debug_assert!(json.is_array());
            let json = GcValue::collect_map([(
                "@list",
                serde_json::from_value::<GcValue>(json.clone()).unwrap(),
            )]);
            let cbor = GcValue::collect_map([(
                21, // "@list" plural
                cbor.clone(),
            )]);
            ("x:lst", json, cbor)
        }
        "p" => {
            let json = serde_json::from_value::<GcValue>(json.clone()).unwrap();
            let cbor = cbor.clone();
            ("x:p", json, cbor)
        }
        _ => todo!("{key}"),
    }
}

fn prepare_expanded_single_json(key: &str, json: &JsonValue) -> GcValue {
    debug_assert!(!json.is_array());
    let json: GcValue = serde_json::from_value(json.clone()).unwrap();
    match key {
        "iri" => GcValue::collect_map([("@id", json)]),
        _ => todo!("{key}"),
    }
}

fn prepare_expanded_single_cbor(key: &str, cbor: &GcValue) -> GcValue {
    match key {
        "iri" => GcValue::collect_map([(4, cbor.clone())]),
        _ => todo!(),
    }
}
