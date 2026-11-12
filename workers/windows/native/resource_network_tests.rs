use super::*;

fn capture() -> Capture {
    Capture {
        start: FILETIME {
            dwHighDateTime: 0,
            dwLowDateTime: 100,
        },
        application_id: b"sealed-native-app".to_vec(),
        listener_port: 45123,
    }
}
fn record() -> Record {
    Record {
        time: 150,
        application_id: b"sealed-native-app".to_vec(),
        listener_port: 45123,
        source_port: 45124,
        local_address: 0x7f000001,
        remote_address: 0x7f000001,
        user_sid: "S-1-5-18".into(),
        loopback: true,
        direction: 0x3900,
        filter_id: 123,
        layer_id: 44,
    }
}

#[test]
fn drop_correlation_rejects_other_sockets_images_times_or_listener_identities() {
    let capture = capture();
    assert!(capture.matches(&record(), 45124, 200));
    for change in 0..11 {
        let mut value = record();
        match change {
            0 => value.time = 99,
            1 => value.time = 201,
            2 => value.application_id.push(0),
            3 => value.listener_port += 1,
            4 => value.source_port += 1,
            5 => value.local_address = 0x7f000002,
            6 => value.remote_address = 0x0100007f,
            7 => value.user_sid = "S-1-5-21-1-2-3-1000".into(),
            8 => value.loopback = false,
            9 => value.direction = 0x3901,
            _ => value.direction = FWP_DIRECTION_INBOUND as u32,
        }
        assert!(
            !capture.matches(&value, 45124, 200),
            "accepted mismatched correlation case {change}"
        );
    }
}

#[test]
fn event_union_is_not_read_without_required_header_fields_or_correct_type() -> Result<()> {
    let mut event = FWPM_NET_EVENT2 {
        r#type: FWPM_NET_EVENT_TYPE_CLASSIFY_DROP,
        ..Default::default()
    };
    assert!(Record::read(&event)?.is_none());
    event.header.flags = u32::MAX;
    event.header.ipVersion = FWP_IP_VERSION_V6;
    assert!(Record::read(&event)?.is_none());
    event.header.ipVersion = FWP_IP_VERSION_V4;
    event.header.ipProtocol = 17;
    assert!(Record::read(&event)?.is_none());
    event.header.ipProtocol = 6;
    event.r#type = FWPM_NET_EVENT_TYPE_CLASSIFY_ALLOW;
    assert!(Record::read(&event)?.is_none());
    event.r#type = FWPM_NET_EVENT_TYPE_CLASSIFY_DROP;
    assert!(Record::read(&event).is_err());
    Ok(())
}

#[test]
fn qualification_evidence_rejects_plain_timeout_and_missing_or_changed_enforcement() -> Result<()> {
    let denied = json!({"method":"socket_permission_denied","source_port":45124,"listener_port":45123,"os_error":10013});
    validate_evidence(&denied)?;
    let evidence = json!({"method":"wfp_appcontainer_loopback_drop","source_port":45124,"listener_port":45123,
        "filter_id":123,"layer_id":44,"event_time":150,"window_start":100,"window_end":200,
        "listener_sid":"S-1-5-18","application_id_matched":true,"loopback":true,"filter":"AppContainerLoopback","action":"block"});
    validate_evidence(&evidence)?;
    for (key, value) in [
        ("method", json!("timed_out")),
        ("source_port", json!(45123)),
        ("listener_port", json!(0)),
        ("filter_id", json!(0)),
        ("layer_id", json!(0)),
        ("event_time", json!(201)),
        ("window_start", json!(201)),
        ("window_end", json!(300_000_101u64)),
        ("listener_sid", json!("S-1-5-19")),
        ("application_id_matched", json!(false)),
        ("loopback", json!(false)),
        ("filter", json!("unrelated block")),
        ("action", json!("allow")),
    ] {
        let mut changed = evidence.clone();
        changed[key] = value;
        assert!(
            validate_evidence(&changed).is_err(),
            "accepted changed enforcement {key}"
        );
    }
    let mut invalid = denied;
    invalid["os_error"] = json!(10060);
    assert!(validate_evidence(&invalid).is_err());
    assert!(
        serde_json::from_value::<Observation>(
            json!({"outcome":"timed_out","source_port":45124,"os_error":null,"passed":true})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn reachable_control_uses_the_actual_listener_and_rejects_an_extra_peer() -> Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    reachable(&listener)?;
    let _extra = TcpStream::connect(listener.local_addr()?)?;
    assert!(reachable(&listener).is_err());
    Ok(())
}
