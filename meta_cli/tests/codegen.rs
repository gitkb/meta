//! The generated types compile for a manifest made to break them
//! (`fixtures/hostile-plugin.toml`; `build.rs` generates them), and read a
//! request's options by the manifest's names.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

mod hostile {
    #![expect(dead_code, reason = "only some of the fields are read")]
    include!(concat!(env!("OUT_DIR"), "/hostile.rs"));
}

use hostile::Command;
use meta_plugin::duration::Seconds;
use serde_json::json;

#[test]
fn a_hostile_manifest_compiles_and_decodes_by_its_own_names() {
    let options = json!({"type": "t", "self": "s", "Self": true, "fn": ["a"], "match": "m",
                         "with-hyphens": "h", "2fa": true});
    let Some(Command::Type(type_)) = Command::decode("type", options).unwrap() else {
        panic!("not the command named type");
    };
    assert_eq!(type_.r#type.as_deref(), Some("t"));
    assert_eq!(type_.self_.as_deref(), Some("s"));
    assert!(type_.Self_);
    assert_eq!(type_.r#fn, ["a"]);
    assert_eq!(type_.r#match, "m");
    assert_eq!(type_.with_hyphens.as_deref(), Some("h"));
    assert!(type_._2fa);
}

#[test]
fn each_option_type_is_its_rust_type() {
    let options = json!({"a-string": "x", "a-uint": 3, "an-int": -1, "a-flag": true,
                         "a-required-path": "/p", "some-paths": ["/a"], "a-duration": 60,
                         "durations": [1, 2], "first": "f",
                         "the-rest": [{"text": "w"}]});
    let decoded = Command::decode("hyphen-ated sub-command", options);
    let Some(Command::HyphenAtedSubCommand(all)) = decoded.unwrap() else {
        panic!("not the command");
    };
    assert_eq!(all.an_int, Some(-1));
    assert_eq!(all.a_uint, 3_usize);
    assert_eq!(all.a_required_path, std::path::Path::new("/p"));
    assert_eq!(all.a_duration, Some(Seconds(60)));
    assert_eq!(all.durations, [Seconds(1), Seconds(2)]);
    assert_eq!(all.the_rest.first().map(|w| w.text.as_str()), Some("w"));
}

#[test]
fn raw_words_arrive_as_words_and_a_name_it_does_not_declare_is_none() {
    let options = json!({"raw": [{"text": "b"}, {"text": "?", "bytes": "/w=="}]});
    let Some(Command::Passthrough(raw)) = Command::decode("passthrough", options).unwrap() else {
        panic!("not the command");
    };
    assert_eq!(raw.raw.len(), 2);
    assert!(Command::decode("nonesuch", json!({})).unwrap().is_none());
    // an option the manifest doesn't declare can't reach the plugin
    assert!(Command::decode("self", json!({"who": 1})).is_err());
}
