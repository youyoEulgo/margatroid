use config_core::{
    CidrBlock, ConfigError, IngressPolicy, LogLevel, MargatroidConfig, OutboundTarget,
};
use core_plugin::Resource;
use server_plugin::HandshakeGuard;
use std::net::{IpAddr, SocketAddr};

fn config(bind: &str) -> MargatroidConfig {
    MargatroidConfig::new(
        bind.parse().unwrap(),
        LogLevel::Info,
        None,
        vec![OutboundTarget::Broadcast],
        vec![OutboundTarget::Type("webui".into())],
    )
    .unwrap()
}

#[test]
fn resource_exposes_validated_configuration_and_owned_targets() {
    fn resource<T: Resource>() {}
    resource::<MargatroidConfig>();
    let value = config("127.0.0.1:3939");
    assert_eq!(value.server_bind(), "127.0.0.1:3939".parse().unwrap());
    assert_eq!(value.server_ingress(), &IngressPolicy::Localhost);
    assert_eq!(value.log_level(), LogLevel::Info);
    assert_eq!(value.log_filter(), None);
    assert_eq!(value.logs(), [OutboundTarget::Broadcast]);
    assert_eq!(
        value.backend_state(),
        [OutboundTarget::Type("webui".into())]
    );
}

#[test]
fn validates_both_target_groups_without_rejecting_cross_group_reuse() {
    for invalid in [
        vec![],
        vec![OutboundTarget::Type("".into())],
        vec![OutboundTarget::Name("   ".into())],
        vec![OutboundTarget::Name("bad\nname".into())],
        vec![OutboundTarget::Type("bad\0type".into())],
    ] {
        for logs_invalid in [true, false] {
            let valid = vec![OutboundTarget::Broadcast];
            let (logs, backend) = if logs_invalid {
                (invalid.clone(), valid)
            } else {
                (valid, invalid.clone())
            };
            assert_eq!(
                MargatroidConfig::new(
                    "127.0.0.1:1".parse().unwrap(),
                    LogLevel::Info,
                    None,
                    logs,
                    backend
                ),
                Err(ConfigError::InvalidTarget)
            );
        }
    }
    let duplicate = vec![OutboundTarget::Broadcast, OutboundTarget::Broadcast];
    assert_eq!(
        MargatroidConfig::new(
            "127.0.0.1:1".parse().unwrap(),
            LogLevel::Info,
            None,
            duplicate.clone(),
            vec![OutboundTarget::Broadcast]
        ),
        Err(ConfigError::DuplicateTarget)
    );
    assert_eq!(
        MargatroidConfig::new(
            "127.0.0.1:1".parse().unwrap(),
            LogLevel::Info,
            None,
            vec![OutboundTarget::Broadcast],
            duplicate
        ),
        Err(ConfigError::DuplicateTarget)
    );
    assert!(
        MargatroidConfig::new(
            "127.0.0.1:1".parse().unwrap(),
            LogLevel::Info,
            None,
            vec![
                OutboundTarget::Type("cli".into()),
                OutboundTarget::Name("cli".into())
            ],
            vec![OutboundTarget::Type("cli".into())]
        )
        .is_ok()
    );
}

#[test]
fn filter_is_nonempty_but_logging_expression_validation_is_deferred() {
    for filter in ["", " \t\n"] {
        assert_eq!(
            MargatroidConfig::new(
                "127.0.0.1:1".parse().unwrap(),
                LogLevel::Info,
                Some(filter.into()),
                vec![OutboundTarget::Broadcast],
                vec![OutboundTarget::Broadcast]
            ),
            Err(ConfigError::InvalidLogFilter)
        );
    }
    let value = MargatroidConfig::new(
        "127.0.0.1:1".parse().unwrap(),
        LogLevel::Off,
        Some("application validates [this".into()),
        vec![OutboundTarget::Broadcast],
        vec![OutboundTarget::Broadcast],
    )
    .unwrap();
    assert_eq!(value.log_filter(), Some("application validates [this"));
    assert_eq!(value.log_level(), LogLevel::Off);
}

#[test]
fn cidr_masks_handle_address_families_and_prefix_boundaries() {
    for (block, accepted, rejected) in [
        ("10.2.3.4/8", "10.255.0.1", "11.0.0.1"),
        ("192.168.1.7", "192.168.1.7", "192.168.1.8"),
        ("2001:db8::/32", "2001:db8::abcd", "2001:db9::1"),
        ("::1", "::1", "::2"),
    ] {
        let block = CidrBlock::parse(block).unwrap();
        assert!(block.contains(accepted.parse().unwrap()));
        assert!(!block.contains(rejected.parse().unwrap()));
    }
    let v4 = CidrBlock::parse("0.0.0.0/0").unwrap();
    assert!(v4.contains("255.255.255.255".parse().unwrap()));
    assert!(!v4.contains("::ffff:127.0.0.1".parse().unwrap()));
    assert!(
        CidrBlock::parse("::/0")
            .unwrap()
            .contains("ffff::1".parse().unwrap())
    );
    assert_eq!(CidrBlock::parse("127.0.0.1").unwrap().prefix(), 32);
    assert_eq!(CidrBlock::parse("::1").unwrap().prefix(), 128);
    assert_eq!(
        CidrBlock::parse("::1").unwrap().network(),
        "::1".parse::<IpAddr>().unwrap()
    );
    for invalid in [
        "",
        "localhost",
        "10.0.0.0/33",
        "::/129",
        "::/-1",
        "::/+1",
        "::/",
        "::/1/2",
    ] {
        assert!(CidrBlock::parse(invalid).is_none(), "{invalid}");
    }
}

#[test]
fn ingress_cannot_be_wider_than_loopback_bind() {
    assert_eq!(
        config("127.0.0.1:1").with_ingress(IngressPolicy::Any),
        Err(ConfigError::InvalidIngress)
    );
    assert_eq!(
        config("[::1]:1").with_ingress(IngressPolicy::Addresses(vec![
            CidrBlock::parse("10.0.0.0/8").unwrap()
        ])),
        Err(ConfigError::InvalidIngress)
    );
    for block in [
        "127.0.0.0/7",
        "::1/127",
        "0.0.0.0/0",
        "::/0",
        "0.0.0.0",
        "::",
    ] {
        assert_eq!(
            config("127.0.0.1:1").with_ingress(IngressPolicy::Addresses(vec![
                CidrBlock::parse(block).unwrap()
            ])),
            Err(ConfigError::InvalidIngress),
            "{block}"
        );
    }
    for block in ["127.0.0.0/8", "::1/128"] {
        assert!(
            config("127.0.0.1:1")
                .with_ingress(IngressPolicy::Addresses(vec![
                    CidrBlock::parse(block).unwrap()
                ]))
                .is_ok()
        );
    }
    assert!(config("0.0.0.0:1").with_ingress(IngressPolicy::Any).is_ok());
    assert!(
        config("[::]:1")
            .with_ingress(IngressPolicy::Addresses(vec![
                CidrBlock::parse("10.0.0.0/8").unwrap()
            ]))
            .is_ok()
    );
}

#[test]
fn nonbrowser_requests_only_check_peer_and_guard_uses_same_policy() {
    let policy = IngressPolicy::Localhost;
    let guard: &dyn HandshakeGuard = &policy;
    for peer in ["127.0.0.1:50000", "[::1]:50000"] {
        assert!(guard.guard(peer.parse().unwrap(), None, None).is_ok());
    }
    let peer: SocketAddr = "10.0.0.5:50000".parse().unwrap();
    let error = guard.guard(peer, None, None).unwrap_err();
    assert!(error.contains("10.0.0.5:50000"));
    assert!(error.contains("localhost"));
    assert!(!IngressPolicy::Addresses(vec![]).allows_peer("127.0.0.1".parse().unwrap()));
}

#[test]
fn browser_origins_require_valid_matching_authorities() {
    let policy = IngressPolicy::Any;
    for (origin, host) in [
        ("http://127.0.0.1:3939", "127.0.0.1:3939"),
        ("https://EXAMPLE.com", "example.COM:443"),
        ("http://example.com:80", "example.com"),
        ("http://[::1]:3939", "[0:0:0:0:0:0:0:1]:3939"),
    ] {
        assert!(
            policy.allows_origin(Some(origin), Some(host)),
            "{origin} {host}"
        );
    }
    for origin in [
        "null",
        "example.com",
        "ftp://example.com",
        "http://",
        "http://evil.example",
        "http://example.com:3000",
        "http://user@example.com",
        "http://example.com/path",
        "http://example.com?query",
        "http://example.com#fragment",
        "http:// example.com",
        "http://example.com\n",
        "http://example.com:",
        "http://example.com:65536",
        "http://example.com:bad",
        "http://[::1",
        "http://::1",
        "http://example.com\\evil",
        "http://exa%mple.com",
        "http://example..com",
        "http://-example.com",
    ] {
        assert!(
            !policy.allows_origin(Some(origin), Some("example.com")),
            "{origin:?}"
        );
    }
    for host in [
        None,
        Some(""),
        Some("user@example.com"),
        Some("example.com/path"),
        Some("example.com:bad"),
    ] {
        assert!(
            !policy.allows_origin(Some("http://example.com"), host),
            "{host:?}"
        );
    }
}

#[test]
fn public_errors_have_useful_display_messages() {
    let errors = [
        ConfigError::ReadFailed("config.toml".into()),
        ConfigError::TooLarge,
        ConfigError::DecodeFailed,
        ConfigError::InvalidBind,
        ConfigError::InvalidIngress,
        ConfigError::InvalidLogLevel,
        ConfigError::InvalidLogFilter,
        ConfigError::InvalidTarget,
        ConfigError::DuplicateTarget,
    ];
    for error in errors {
        let error: &dyn std::error::Error = &error;
        assert!(!error.to_string().is_empty());
    }
}
