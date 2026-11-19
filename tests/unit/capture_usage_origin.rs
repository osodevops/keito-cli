//! Pure bounded metadata contracts; no OS processes are observed here.
use super::*;
fn node(pid: i32) -> Snapshot {
    Snapshot {
        pid,
        parent: pid + 1,
        started: (10, 20),
        image: PathBuf::from("/inert/owned-image"),
    }
}
#[test]
fn first_and_eighth_ancestor_are_the_only_bounded_positive_edges() {
    let file = tempfile::tempfile().unwrap();
    for matched in [1, 8] {
        let mut calls = 0;
        let result = collect_chain(
            1,
            |pid| {
                calls += 1;
                Some((node(pid), file.try_clone().unwrap()))
            },
            |node, _| node.pid == matched,
        );
        assert!(result.is_some());
        assert_eq!(calls, matched);
    }
}
#[test]
fn ninth_ancestor_wrong_image_missing_and_invalid_pid_fail_closed() {
    let file = tempfile::tempfile().unwrap();
    for matched in [9, 99] {
        let mut calls = 0;
        assert!(collect_chain(
            1,
            |pid| {
                calls += 1;
                Some((node(pid), file.try_clone().unwrap()))
            },
            |node, _| node.pid == matched
        )
        .is_none());
        assert_eq!(calls, 8);
    }
    assert!(collect_chain(1, |_| None, |_, _| true).is_none());
    for pid in [0, -1] {
        assert!(collect_chain(pid, |_| panic!("invalid PID was observed"), |_, _| true).is_none());
    }
}
#[test]
fn observed_pid_zero_start_self_parent_and_cycles_are_refused() {
    let file = tempfile::tempfile().unwrap();
    for field in [0, 1, 2] {
        assert!(collect_chain(
            1,
            |pid| {
                let mut value = node(pid);
                match field {
                    0 => value.pid += 1,
                    1 => value.started.0 = 0,
                    _ => value.parent = pid,
                };
                Some((value, file.try_clone().unwrap()))
            },
            |_, _| true
        )
        .is_none());
    }
    let mut calls = 0;
    assert!(collect_chain(
        1,
        |pid| {
            calls += 1;
            let mut value = node(pid);
            if pid == 2 {
                value.parent = 1
            };
            Some((value, file.try_clone().unwrap()))
        },
        |_, _| false
    )
    .is_none());
    assert_eq!(calls, 2);
}
#[test]
fn revalidation_rejects_changed_pid_start_parent_image_missing_or_chain() {
    let file = tempfile::tempfile().unwrap();
    let nodes = vec![node(1), node(2)];
    assert!(stable_chain(&nodes, 1, |pid| Some((
        node(pid),
        file.try_clone().unwrap()
    ))));
    assert!(!stable_chain(&nodes, 9, |_| panic!(
        "changed immediate parent was observed"
    )));
    assert!(!stable_chain(&nodes, 1, |_| None));
    for field in [0, 1, 2, 3] {
        assert!(!stable_chain(&nodes, 1, |pid| {
            let mut value = node(pid);
            match field {
                0 => value.pid += 10,
                1 => value.started.1 += 1,
                2 => value.parent += 10,
                _ => value.image = PathBuf::from("/inert/replaced-image"),
            };
            Some((value, file.try_clone().unwrap()))
        }));
    }
    let mut broken = nodes.clone();
    broken[0].parent = 3;
    assert!(!stable_chain(&broken, 1, |_| panic!(
        "broken stored chain was observed"
    )));
}
