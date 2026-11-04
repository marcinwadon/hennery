//! Frozen release inputs and the actual linked crypto library for diagnostics.
use std::sync::OnceLock;

pub fn long_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        format!(
            "{}\nsource: {}\nUI SHA-256: {}\nadapter manifest SHA-256: {}\ncrypto: {}",
            env!("CARGO_PKG_VERSION"),
            option_env!("HENNERY_BUILD_SOURCE_SHA").unwrap_or("unrecorded (development build)"),
            option_env!("HENNERY_BUILD_UI_SHA256").unwrap_or("unrecorded (development build)"),
            option_env!("HENNERY_BUILD_MANIFEST_SHA256").unwrap_or("unrecorded (development build)"),
            openssl::version::version()
        )
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostics_include_every_identity_field() {
        let version = super::long_version();
        assert!(version.starts_with(env!("CARGO_PKG_VERSION")));
        for field in ["source: ", "UI SHA-256: ", "adapter manifest SHA-256: "] {
            assert!(version.contains(field), "missing {field}");
        }
        assert_eq!(
            version.lines().find_map(|line| line.strip_prefix("crypto: ")),
            Some(openssl::version::version())
        );
    }
}
