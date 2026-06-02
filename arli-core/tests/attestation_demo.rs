use arli_core::attestation::{ArliKeypair, AttestationBuilder};
use std::path::Path;

#[test]
fn demo_real_attestation() {
    let key_path = Path::new("/home/paperclip/.arli/arli_key.pem");
    if !key_path.exists() {
        eprintln!("Key not found at {:?}, skipping", key_path);
        return;
    }
    let kp = ArliKeypair::load(key_path).expect("load key");
    
    let builder = AttestationBuilder::new(
        kp,
        "308a40914f3397565e5637071a904202e8ca4619a638e4382d54491219fc1047".into(),
    );
    
    let ocsf = r#"{"event_type":"sandbox.create","sandbox_id":"test-001","outcome":"created"}"#;
    let att = builder.build(
        "run-test-001".into(),
        "agent_a4ce04edba2b82253f27fb6b22bbd562".into(),
        "job-test-001".into(),
        ocsf,
        None,
        "sha256:test-policy-v1".into(),
        true, true, 65534,
    );
    
    assert!(att.verify());
    let json = serde_json::to_string(&att).unwrap();
    println!("ATTESTATION_JSON_START{}ATTESTATION_JSON_END", json);
}
