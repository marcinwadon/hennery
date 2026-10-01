//! Passkeys (kernel spec §3.2, §11; plan 3c), with a software passkey in
//! place of a browser and its authenticator (decision 10).

use hennery_kernel::operator::{Operator, PublicUrl};
use hennery_kernel::passkeys::{relying_party, user_handle};
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{Url, Uuid};

/// A software passkey that claims user verification, as a platform
/// authenticator does after a fingerprint or a PIN.
fn authenticator() -> WebauthnAuthenticator<SoftPasskey> {
    WebauthnAuthenticator::new(SoftPasskey::new(true))
}

/// Decision 1: the RP id is `public_url`'s host, in lowercase; an IP
/// address has none.
#[test]
fn the_relying_party_is_the_public_urls_host() {
    for (public_url, rp_id) in [
        ("https://hennery.example", Some("hennery.example")),
        ("https://Hennery.Example:8443/", Some("hennery.example")),
        ("http://localhost:7117", Some("localhost")),
        ("http://127.0.0.1:7117", None),
        ("http://[::1]:7117", None),
        ("https://192.0.2.1", None),
    ] {
        let url = PublicUrl::parse(public_url).unwrap();
        assert_eq!(url.rp_id(), rp_id, "{public_url}");
        assert_eq!(relying_party(&url).is_some(), rp_id.is_some(), "{public_url}");
    }
}

/// Kernel spec §3.2: "subdomains and arbitrary ports not allowed". A
/// passkey made at `public_url`'s own origin registers; one made at a
/// subdomain, at another port or over another scheme does not.
#[test]
fn only_the_public_urls_own_origin_registers_a_passkey() {
    for (public_url, origin, registers) in [
        ("https://hennery.example", "https://hennery.example", true),
        ("https://hennery.example", "https://sub.hennery.example", false),
        ("https://hennery.example", "https://hennery.example:8443", false),
        ("http://localhost:7117", "http://localhost:7117", true),
        ("http://localhost:7117", "http://localhost:7118", false),
        ("https://localhost", "http://localhost", false),
    ] {
        let rp = relying_party(&PublicUrl::parse(public_url).unwrap()).unwrap();
        let (options, state) = rp
            .start_passkey_registration(Uuid::nil(), "owner", "owner", None)
            .unwrap();
        let credential = authenticator()
            .do_registration(Url::parse(origin).unwrap(), options)
            .unwrap();
        assert_eq!(
            rp.finish_passkey_registration(&credential, &state).is_ok(),
            registers,
            "{origin} for {public_url}"
        );
    }
}

/// Decision 3: the user handle is the owner's, the same every time, and
/// another owner's differs.
#[test]
fn the_user_handle_is_derived_from_the_owner() {
    assert_eq!(user_handle("owner-a"), user_handle("owner-a"));
    assert_ne!(user_handle("owner-a"), user_handle("owner-b"));
    assert_ne!(user_handle("owner-a"), Uuid::nil());
}

/// Decision 3: the `passkeys` table carries `owner_id` from its first
/// migration, a foreign key to `owners`, and a credential id unique across
/// owners.
#[test]
fn the_passkeys_table_carries_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    Operator::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let owner_id_not_null: bool = conn
        .query_row(
            "SELECT \"notnull\" FROM pragma_table_info('passkeys') WHERE name = 'owner_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(owner_id_not_null);
    let parent: String = conn
        .query_row(
            "SELECT \"table\" FROM pragma_foreign_key_list('passkeys') WHERE \"from\" = 'owner_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parent, "owners");
    let unique: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_index_list('passkeys') l
                 JOIN pragma_index_info(l.name) i WHERE l.\"unique\" = 1 AND i.name = 'credential_id')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(unique);
}
