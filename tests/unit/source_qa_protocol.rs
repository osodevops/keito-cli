//! Socket-only state-machine acceptance; no processes, signalling or profiles.
use super::*;
use std::net::Shutdown;
const NONCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PEER: u32 = 12345;

fn pair() -> (Endpoint, UnixStream) {
    let (reader, writer) = UnixStream::pair().unwrap();
    (
        Endpoint::new(reader, NONCE.into(), Some(PEER)).unwrap(),
        writer,
    )
}
fn frame(writer: &mut UnixStream, nonce: &str, value: &str) {
    writeln!(writer, "{nonce}|{value}").unwrap();
}
fn live(endpoint: &mut Endpoint, writer: &mut UnixStream) {
    frame(writer, NONCE, "LIVE|12345");
    endpoint.receive(false).unwrap();
}
#[test]
fn valid_terminal_and_eof_control() {
    let (mut endpoint, mut writer) = pair();
    live(&mut endpoint, &mut writer);
    frame(&mut writer, NONCE, "SETTLED|12345");
    writer.shutdown(Shutdown::Write).unwrap();
    endpoint.receive(false).unwrap();
    assert!(endpoint.settled);
}
#[test]
fn invalid_nonce_is_sticky_across_later_settlement() {
    let (mut endpoint, mut writer) = pair();
    live(&mut endpoint, &mut writer);
    frame(
        &mut writer,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "LIVE|12345",
    );
    let first = endpoint.receive(false).unwrap_err().to_string();
    frame(&mut writer, NONCE, "SETTLED|12345");
    writer.shutdown(Shutdown::Write).unwrap();
    assert_eq!(endpoint.receive(false).unwrap_err().to_string(), first);
    assert!(!endpoint.settled);
}
#[test]
fn unmatched_wait_is_sticky_across_later_settlement() {
    let (mut endpoint, mut writer) = pair();
    live(&mut endpoint, &mut writer);
    frame(
        &mut writer,
        NONCE,
        "WAITED|12345|12346|bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb|0",
    );
    let first = endpoint.receive(false).unwrap_err().to_string();
    frame(&mut writer, NONCE, "SETTLED|12345");
    writer.shutdown(Shutdown::Write).unwrap();
    assert_eq!(endpoint.receive(false).unwrap_err().to_string(), first);
    assert!(!endpoint.settled);
}
#[test]
fn terminal_requires_eof_before_it_counts_as_settled() {
    let (mut endpoint, mut writer) = pair();
    live(&mut endpoint, &mut writer);
    frame(&mut writer, NONCE, "SETTLED|12345");
    endpoint.receive(false).unwrap();
    assert!(!endpoint.settled);
    writer.shutdown(Shutdown::Write).unwrap();
    endpoint.receive(false).unwrap();
    assert!(endpoint.settled);
}
#[test]
fn registration_requires_live_owner() {
    let (mut endpoint, mut writer) = pair();
    frame(
        &mut writer,
        NONCE,
        "REGISTER|12345|12346|bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb|1",
    );
    assert!(endpoint.receive(false).is_err());
}
#[test]
fn frame_limit_applies_to_complete_lines() {
    let (mut endpoint, mut writer) = pair();
    live(&mut endpoint, &mut writer);
    frame(
        &mut writer,
        NONCE,
        &format!(
            "REGISTER|12345|12346|bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb|{}",
            "9".repeat(300)
        ),
    );
    assert!(endpoint.receive(false).is_err());
}

#[test]
fn all_control_faults_poison_owner_and_endpoint() {
    for value in ["LIVE|12345", "FAILED|12345", "LIVE|12346", "unknown|12345"] {
        let (mut endpoint, mut writer) = pair();
        live(&mut endpoint, &mut writer);
        frame(&mut writer, NONCE, value);
        let first = endpoint.receive(false).unwrap_err();
        assert_eq!(endpoint.owner.borrow().first_fault, Some(first));
        assert!(endpoint.owner.borrow().admission_closed);
        frame(&mut writer, NONCE, "SETTLED|12345");
        writer.shutdown(Shutdown::Write).unwrap();
        assert_eq!(endpoint.receive(false).unwrap_err(), first);
        assert!(!endpoint.is_settled());
    }
}
#[test]
fn trailing_data_never_completes_a_terminal_proof() {
    for trailing in [
        "trailing",
        "LIVE|12345\n",
        "SETTLED|12345\n",
        "WAITED|12345\n",
    ] {
        let (mut endpoint, mut writer) = pair();
        live(&mut endpoint, &mut writer);
        frame(&mut writer, NONCE, "SETTLED|12345");
        endpoint.receive(false).unwrap();
        writer.write_all(trailing.as_bytes()).unwrap();
        writer.shutdown(Shutdown::Write).unwrap();
        assert!(endpoint.receive(false).is_err());
        assert!(!endpoint.is_settled());
    }
}
#[test]
fn eof_before_live_is_cancellation_only_for_parent_role() {
    let (mut endpoint, writer) = pair();
    writer.shutdown(Shutdown::Write).unwrap();
    assert!(endpoint.receive(true).unwrap());
    assert!(endpoint.owner.borrow().admission_closed);
    assert!(!endpoint.is_settled());
    let (mut endpoint, writer) = pair();
    writer.shutdown(Shutdown::Write).unwrap();
    assert!(endpoint.receive(false).is_err());
}
#[test]
fn received_cancel_deadline_only_gets_earlier() {
    let (mut endpoint, mut writer) = pair();
    for deadline in [100, 200, 50] {
        frame(&mut writer, NONCE, &format!("CANCEL|12345|{deadline}"));
        assert!(endpoint.receive(true).unwrap());
    }
    assert_eq!(endpoint.cancel_deadline_ns(), Some(50));
}
#[test]
fn shared_owner_fault_blocks_clean_upstream_terminal() {
    let owner: SharedOwner = Rc::default();
    let (read, mut peer) = UnixStream::pair().unwrap();
    let mut downstream =
        Endpoint::with_owner(read, NONCE.into(), Some(PEER), owner.clone()).unwrap();
    live(&mut downstream, &mut peer);
    frame(&mut peer, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "LIVE|12345");
    let first = downstream.receive(false).unwrap_err();
    let (read, mut upstream_peer) = UnixStream::pair().unwrap();
    let mut upstream = Endpoint::with_owner(read, NONCE.into(), None, owner.clone()).unwrap();
    upstream.send_live().unwrap();
    assert!(upstream.send_terminal(true).is_err());
    assert_eq!(owner.borrow().first_fault, Some(first));
    upstream.send_terminal(false).unwrap();
    let mut trace = String::new();
    upstream_peer.read_to_string(&mut trace).unwrap();
    assert!(trace.contains("|FAILED|"));
    assert!(!trace.contains("|SETTLED|"));
}
#[test]
fn budgets_are_total_sticky_and_checked_before_increment() {
    for (kind, cap) in [
        ("wire", MAX_WIRE),
        ("frames", MAX_FRAMES),
        ("registrations", MAX_REGISTRATIONS),
    ] {
        let mut state = OwnerState::default();
        state.charge(kind, cap).unwrap();
        let first = state.charge(kind, 1).unwrap_err();
        assert_eq!(state.charge(kind, 0).unwrap_err(), first);
        assert_eq!(state.first_fault, Some(first));
        assert!(state.admission_closed);
    }
}
#[test]
fn each_poll_has_fixed_read_and_parse_budgets() {
    let (mut endpoint, mut writer) = pair();
    for _ in 0..100 {
        frame(&mut writer, NONCE, "CANCEL|12345|100");
    }
    endpoint.receive(true).unwrap();
    assert!(endpoint.last_poll.reads <= MAX_READS);
    assert!(endpoint.last_poll.parsed <= MAX_PARSE);
    assert!(endpoint.owner.borrow().frames < 100);
    assert!(endpoint.pending.len() <= MAX_PENDING);
}
#[test]
fn live_registration_trace_round_trip_and_shared_budget() {
    let owner: SharedOwner = Rc::default();
    let (left, right) = UnixStream::pair().unwrap();
    let mut sender = Endpoint::with_owner(left, NONCE.into(), None, owner.clone()).unwrap();
    let mut receiver =
        Endpoint::with_owner(right, NONCE.into(), Some(std::process::id()), owner.clone()).unwrap();
    sender.send_live().unwrap();
    let sent_wire = owner.borrow().wire;
    receiver.receive(false).unwrap();
    assert_eq!(owner.borrow().wire, sent_wire * 2);
    sender
        .metadata(&format!(
            "REGISTER|{}|12346|bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb|1",
            std::process::id()
        ))
        .unwrap();
    receiver.receive(false).unwrap();
    assert_eq!(owner.borrow().registrations, 2);
    sender
        .metadata(&format!(
            "WAITED|{}|12346|bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb|0",
            std::process::id()
        ))
        .unwrap();
    receiver.receive(false).unwrap();
    assert_eq!(receiver.take_events().len(), 2);
    sender.send_terminal(true).unwrap();
    receiver.receive(false).unwrap();
    assert!(receiver.is_settled());
}
#[test]
fn invalid_or_already_closed_owner_edges_are_refused() {
    for owner in ["9999", "12346"] {
        let (mut endpoint, mut writer) = pair();
        live(&mut endpoint, &mut writer);
        frame(
            &mut writer,
            NONCE,
            "REGISTER|12345|12346|bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb|1",
        );
        frame(
            &mut writer,
            NONCE,
            "WAITED|12345|12346|bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb|0",
        );
        frame(
            &mut writer,
            NONCE,
            &format!("REGISTER|{owner}|12347|cccccccccccccccccccccccccccccccc|1"),
        );
        assert!(endpoint.receive(false).is_err());
    }
}
#[test]
fn malformed_numeric_fields_are_sticky_and_never_echo_raw_input() {
    for value in ["--1", "+1", "001", "PRIVATE_CANARY"] {
        let (mut endpoint, mut writer) = pair();
        live(&mut endpoint, &mut writer);
        frame(
            &mut writer,
            NONCE,
            &format!("REGISTER|{value}|12346|bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb|1"),
        );
        let error = endpoint.receive(false).unwrap_err();
        assert!(!error.to_string().contains(value));
        assert_eq!(endpoint.receive(false).unwrap_err(), error);
    }
}

#[test]
fn constructor_refusal_poisons_supplied_owner() {
    let owner: SharedOwner = Rc::default();
    let (left, _right) = UnixStream::pair().unwrap();
    assert!(Endpoint::with_owner(left, "invalid".into(), Some(PEER), owner.clone()).is_err());
    assert_eq!(
        owner.borrow().first_fault,
        Some(ProtocolError("ENDPOINT_SCHEMA"))
    );
    assert!(owner.borrow().check_admission().is_err());
}

#[test]
fn shared_wire_contract_vectors() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/source_qa_protocol_vectors.json")).unwrap();
    assert_eq!(corpus["schema"], 1);
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 16);
    for case in cases {
        let (mut endpoint, mut writer) = pair();
        let parent = case["parent"].as_bool().unwrap();
        let mut fault = false;
        for chunk in case["chunks"].as_array().unwrap() {
            writer
                .write_all(chunk.as_str().unwrap().as_bytes())
                .unwrap();
            fault |= endpoint.receive(parent).is_err();
        }
        if case["close_writer"].as_bool().unwrap() {
            writer.shutdown(Shutdown::Write).unwrap();
        }
        fault |= endpoint.receive(parent).is_err();
        assert_eq!(fault, case["expected"] == "fault", "{}", case["name"]);
        assert_eq!(
            endpoint.is_settled(),
            case["expected"] == "clean",
            "{}",
            case["name"]
        );
        if case["expected"] == "cancel" {
            assert!(endpoint.cancelled, "{}", case["name"]);
        }
    }
}
