use super::*;
use tokio::time::Duration;
use whatsapp_rust::NodeBuilder;

const CALL: &str = "00synthetic0000000000000000000001";

fn enc(kind: &'static str) -> Node {
    NodeBuilder::new("enc")
        .attr("v", "2")
        .attr("type", kind)
        .bytes(vec![1, 2, 3])
        .build()
}

fn to(device: &str, kind: &'static str) -> Node {
    NodeBuilder::new("to")
        .attr("jid", device)
        .children([enc(kind)])
        .build()
}

fn control(kind: &'static str, call: &str, reason: Option<&str>) -> Node {
    let mut child = NodeBuilder::new(kind).attr("call-id", call);
    if let Some(reason) = reason {
        child = child.attr("reason", reason);
    }
    NodeBuilder::new("call")
        .attr("from", "100000000000001:2@lid")
        .children([child.build()])
        .build()
}

fn ringing(call: &str, device: &str) -> Node {
    NodeBuilder::new("receipt")
        .attr("from", device)
        .attr("id", "R1")
        .children([NodeBuilder::new("offer").attr("call-id", call).build()])
        .build()
}

#[test]
fn call_replies_are_classified_without_keeping_other_stanzas() {
    let classify = |node: Node| reply(&node.as_node_ref());
    assert_eq!(
        classify(
            NodeBuilder::new("ack")
                .attr("class", "call")
                .attr("id", "S1")
                .attr("error", "463")
                .build()
        ),
        Some(Reply::Ack {
            stanza: "S1".into(),
            error: Some("463".into())
        })
    );
    assert_eq!(
        classify(
            NodeBuilder::new("ack")
                .attr("class", "message")
                .attr("id", "S1")
                .build()
        ),
        None
    );
    assert_eq!(
        classify(ringing(CALL, "100000000000001:3@lid")),
        Some(Reply::Ringing {
            call: CALL.into(),
            device: "100000000000001:3@lid".into()
        })
    );
    assert_eq!(
        classify(
            NodeBuilder::new("receipt")
                .attr("type", "enc_rekey_retry")
                .children([NodeBuilder::new("enc_rekey")
                    .attr("call-id", CALL)
                    .attr("count", "1")
                    .build()])
                .build()
        ),
        Some(Reply::KeyRequest { call: CALL.into() })
    );
    assert_eq!(
        classify(control("terminate", CALL, Some("timeout"))),
        Some(Reply::Control {
            call: CALL.into(),
            kind: "terminate",
            reason: Some("timeout".into())
        })
    );
    // Unexpected reason text is not copied into the journal.
    assert_eq!(
        classify(control("reject", CALL, Some("not a protocol word"))),
        Some(Reply::Control {
            call: CALL.into(),
            kind: "reject",
            reason: Some("other".into())
        })
    );
    // A message delivery receipt and call transport traffic are ignored.
    assert_eq!(
        classify(NodeBuilder::new("receipt").attr("id", "M1").build()),
        None
    );
    assert_eq!(classify(control("transport", CALL, None)), None);
}

#[test]
fn the_offer_shape_counts_addressed_devices_new_sessions_and_the_token() {
    let multi = NodeBuilder::new("call")
        .attr("id", "S1")
        .children([NodeBuilder::new("offer")
            .attr("call-id", CALL)
            .children([
                NodeBuilder::new("privacy").bytes(vec![9]).build(),
                NodeBuilder::new("net").attr("medium", "3").build(),
                NodeBuilder::new("destination")
                    .children([
                        to("100000000000001:0@lid", "pkmsg"),
                        to("100000000000001:2@lid", "msg"),
                        to("100000000000001:3@lid", "msg"),
                    ])
                    .build(),
            ])
            .build()])
        .build();
    assert_eq!(
        offer(&multi.as_node_ref()),
        Some(Offer {
            stanza: Some("S1".into()),
            devices: 3,
            new_sessions: 1,
            privacy: true
        })
    );
    let single = NodeBuilder::new("call")
        .children([NodeBuilder::new("offer")
            .attr("call-id", CALL)
            .children([enc("msg")])
            .build()])
        .build();
    assert_eq!(
        offer(&single.as_node_ref()),
        Some(Offer {
            stanza: None,
            devices: 1,
            new_sessions: 0,
            privacy: false
        })
    );
    assert_eq!(offer(&control("terminate", CALL, None).as_node_ref()), None);
}

#[test]
fn the_summary_reports_only_this_calls_replies_in_milliseconds() {
    let began = Instant::now();
    let sent = began + Duration::from_millis(40);
    let at = |ms| sent + Duration::from_millis(ms);
    let ringing = |device: &str| Reply::Ringing {
        call: CALL.into(),
        device: device.into(),
    };
    let replies = vec![
        (
            Reply::Ack {
                stanza: "S1".into(),
                error: None,
            },
            at(120),
        ),
        // Another call's ack and replies do not count.
        (
            Reply::Ack {
                stanza: "S2".into(),
                error: Some("463".into()),
            },
            at(130),
        ),
        (ringing("100000000000001:3@lid"), at(900)),
        (ringing("100000000000001:3@lid"), at(950)),
        (ringing("100000000000001:0@lid"), at(1100)),
        (
            Reply::Ringing {
                call: "00other".into(),
                device: "100000000000009:0@lid".into(),
            },
            at(1200),
        ),
        (Reply::KeyRequest { call: CALL.into() }, at(1300)),
        (
            Reply::Control {
                call: CALL.into(),
                kind: "terminate",
                reason: Some("timeout".into()),
            },
            at(45_000),
        ),
    ];
    let offer = Offer {
        stanza: Some("S1".into()),
        devices: 2,
        new_sessions: 1,
        privacy: false,
    };
    let line = summarize(
        began,
        Some((sent, CALL.into(), Some(offer))),
        replies,
        "ended",
    );
    assert!(
        line.starts_with(
            "Heya call trace: offer prepared in 40 ms for 2 devices (1 new sessions), \
             privacy token missing; server ack after 120 ms; ringing receipts from 2 devices \
             (first after 900 ms); call-key requests 1 (first after 1300 ms); \
             callee terminate (timeout) after 45000 ms; setup ended after "
        ),
        "{line}"
    );
    assert!(!line.contains(CALL) && !line.contains("@lid"), "{line}");

    let line = summarize(
        began,
        Some((sent, CALL.into(), None)),
        Vec::new(),
        "hung up",
    );
    assert!(
        line.contains(
            ", offer stanza not captured; no server ack matched; ringing receipts from 0 devices; \
             call-key requests 0; no callee control; setup hung up after "
        ),
        "{line}"
    );
    let line = summarize(began, None, Vec::new(), "failed");
    assert!(
        line.starts_with("Heya call trace: no offer sent; setup failed after "),
        "{line}"
    );
}
