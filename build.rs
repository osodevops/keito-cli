fn main() {
    println!("cargo:rerun-if-env-changed=KEITO_RELEASE_GUARD_SHA256");
    println!(
        "cargo:rustc-env=KEITO_BUILD_TARGET={}",
        std::env::var("TARGET").expect("Cargo target")
    );
    if let Ok(hash) = std::env::var("KEITO_RELEASE_GUARD_SHA256") {
        assert!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid final guard release hash"
        );
        println!("cargo:rustc-env=KEITO_RELEASE_GUARD_SHA256={hash}");
    }
}
