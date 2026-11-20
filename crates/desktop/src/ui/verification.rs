#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::net::TcpListener;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers, accesskit};
use fix_codec::Decoder;
use fix_dict::Version;
use fix_session::Role;
use fixtester_desktop::model::Profile;

use super::Workspace;

#[path = "../../tests/support/wire.rs"]
mod wire;
use wire::Wire;

struct Driver {
    workspace: Workspace,
    context: egui::Context,
    nodes: Vec<(accesskit::NodeId, accesskit::Node)>,
}

impl Driver {
    fn new(path: &Path) -> Self {
        let context = egui::Context::default();
        super::style(&context);
        context.enable_accesskit();
        let mut driver = Self {
            workspace: Workspace::open(path, None).unwrap(),
            context,
            nodes: Vec::new(),
        };
        driver.frame(Vec::new(), Modifiers::NONE);
        driver
    }

    fn frame(&mut self, mut events: Vec<Event>, modifiers: Modifiers) {
        events.insert(0, Event::ModifiersChanged(modifiers));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 760.0),
            )),
            events,
            ..Default::default()
        };
        let mut result = self.context.run_ui(input, |ui| self.workspace.draw(ui));
        result.textures_delta.clear();
        self.nodes = result.platform_output.accesskit_update.unwrap().nodes;
    }

    fn find(&self, role: accesskit::Role, text: &str) -> Option<accesskit::NodeId> {
        self.nodes.iter().find_map(|(id, node)| {
            let matches = |node: &accesskit::Node| {
                node.label()
                    .or_else(|| node.value())
                    .is_some_and(|value| value.contains(text))
            };
            let has_label = node.labelled_by().iter().any(|label| {
                self.nodes
                    .iter()
                    .any(|(id, node)| id == label && matches(node))
            });
            (node.role() == role && (has_label || matches(node))).then_some(*id)
        })
    }

    fn wait_for(&mut self, role: accesskit::Role, text: &str) -> accesskit::NodeId {
        let since = Instant::now();
        loop {
            self.frame(Vec::new(), Modifiers::NONE);
            if let Some(node) = self.find(role, text) {
                return node;
            }
            assert!(
                since.elapsed() < Duration::from_secs(4),
                "No {role:?} containing {text:?}"
            );
            thread::sleep(Duration::from_millis(16));
        }
    }

    fn action(&mut self, id: accesskit::NodeId, action: accesskit::Action) {
        self.frame(
            vec![Event::AccessKitActionRequest(accesskit::ActionRequest {
                action,
                target_node: id,
                target_tree: accesskit::TreeId::ROOT,
                data: None,
            })],
            Modifiers::NONE,
        );
    }

    fn click(&mut self, text: &str) {
        let id = self.wait_for(accesskit::Role::Button, text);
        self.action(id, accesskit::Action::Click);
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        for pressed in [true, false] {
            self.frame(
                vec![Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers,
                }],
                modifiers,
            );
        }
    }

    fn enter(&mut self, label: &str, text: &str) {
        let id = self.wait_for(accesskit::Role::TextInput, label);
        self.action(id, accesskit::Action::Focus);
        self.replace_text(text);
    }

    fn enter_additional_tags(&mut self, text: &str) {
        let disclosure = self.wait_for(accesskit::Role::DisclosureTriangle, "Additional FIX tags");
        self.action(disclosure, accesskit::Action::Click);
        self.enter("Tag=Value pairs separated by semicolons", text);
    }

    fn wait_for_workspace(&mut self) {
        let since = Instant::now();
        loop {
            self.frame(Vec::new(), Modifiers::NONE);
            if self
                .context
                .memory(|memory| memory.top_modal_layer().is_none())
            {
                return;
            }
            assert!(
                since.elapsed() < Duration::from_secs(4),
                "The dialog did not close."
            );
            thread::sleep(Duration::from_millis(16));
        }
    }

    fn assert_selected_reference(&mut self, reference: &[u8]) {
        self.wait_for(accesskit::Role::Label, "OrigClOrdID (read-only)");
        self.wait_for(
            accesskit::Role::Label,
            std::str::from_utf8(reference).unwrap(),
        );
        assert!(
            self.find(accesskit::Role::TextInput, "OrigClOrdID")
                .is_none()
        );
    }

    fn replace_text(&mut self, text: &str) {
        self.key(Key::A, Modifiers::COMMAND);
        self.frame(vec![Event::Text(text.into())], Modifiers::NONE);
    }
}

#[test]
fn workspace_new_report_replace_and_cancel() {
    order_flow(false);
}

#[test]
fn workspace_price_only_replace_keeps_selected_instructions() {
    order_flow(true);
}

fn connected_workspace(path: &Path) -> (Driver, Wire) {
    connected_workspace_version(path, Version::Fix44)
}

fn connected_workspace_version(path: &Path, version: Version) -> (Driver, Wire) {
    let addr = TcpListener::bind("127.0.0.1:0").unwrap();
    let profile = Profile {
        name: "Loopback example".into(),
        version,
        role: Role::Initiator,
        sender: "TRAINER".into(),
        target: "VENUE".into(),
        address: addr.local_addr().unwrap(),
        heartbeat: 30,
    };
    let mut ui = Driver::new(path);
    ui.click("Add profile");
    if version != Version::Fix44 {
        let dropdown = ui.wait_for(accesskit::Role::ComboBox, "FIX.4.4");
        ui.action(dropdown, accesskit::Action::Click);
        ui.click(version.begin_string());
    }
    ui.enter("Name", &profile.name);
    ui.enter("SenderCompID", &profile.sender);
    ui.enter("TargetCompID", &profile.target);
    ui.enter("Peer address", &profile.address.to_string());
    ui.click("Save profile");
    ui.wait_for_workspace();
    ui.key(Key::F5, Modifiers::NONE);
    addr.set_nonblocking(true).unwrap();
    let start = Instant::now();
    let socket = loop {
        match addr.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    start.elapsed() < Duration::from_secs(4),
                    "FIXTester did not connect."
                );
                thread::sleep(Duration::from_millis(15));
            }
            Err(error) => panic!("{error}"),
        }
    };
    let mut peer = Wire::new(socket, &profile);
    peer.receive(b"A", 1);
    peer.logon(1, false);
    ui.wait_for(accesskit::Role::Label, "Logon complete.");
    (ui, peer)
}

#[test]
fn workspace_sends_a_multileg_order_and_an_order_list() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ui, mut peer) = connected_workspace(&dir.path().join("grouped.db"));

    ui.click("Multileg order");
    ui.enter("Strategy symbol", "FUT-SPREAD");
    ui.enter("Strategy quantity", "1");
    ui.enter("Net limit price", "-0.25");
    ui.enter("Strategy CFI override", "FMXXXX");
    ui.enter("Leg 1 symbol", "FUT-Z");
    ui.enter("Leg 1 maturity", "202712");
    ui.enter("Leg 2 symbol", "FUT-H");
    ui.enter("Leg 2 maturity", "202803");
    ui.enter_additional_tags("9001=Strategy");
    ui.click("Send");

    let raw = peer.receive(b"AB", 2);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    assert_eq!(fields.get_u64(555), Some(2));
    assert_eq!(fields.get(9001), Some(b"Strategy".as_slice()));
    assert!(
        fields.iter().position(|(tag, _)| tag == 9001).unwrap()
            < fields.iter().position(|(tag, _)| tag == 555).unwrap()
    );
    assert_eq!(fields.get(461), Some(b"FMXXXX".as_slice()));
    assert_eq!(fields.get(44), Some(b"-0.25".as_slice()));
    ui.click("Order list");
    ui.enter("Symbol", "ALPHA");
    ui.enter("Quantity", "12");
    ui.enter("Limit price", "19.50");
    ui.enter_additional_tags("9002=List");
    ui.click("Send");

    let raw = peer.receive(b"E", 3);
    let fields = decoder.decode(&raw).unwrap().fields();
    assert_eq!(fields.get_u64(73), Some(1));
    assert_eq!(fields.get(9001), None);
    assert_eq!(fields.get(9002), Some(b"List".as_slice()));
    assert!(
        fields.iter().position(|(tag, _)| tag == 9002).unwrap()
            < fields.iter().position(|(tag, _)| tag == 73).unwrap()
    );
    assert_eq!(fields.get(55), Some(b"ALPHA".as_slice()));
    assert_eq!(fields.get(44), Some(b"19.50".as_slice()));
}

fn order_flow(from_tags: bool) {
    let dir = tempfile::tempdir().unwrap();
    let (mut ui, mut peer) = connected_workspace(&dir.path().join("workspace.db"));

    if from_tags {
        ui.key(Key::T, Modifiers::CTRL);
        ui.enter("MsgType", "D");
        let editor = ui.wait_for(
            accesskit::Role::MultilineTextInput,
            "One Tag=Value pair per line.",
        );
        ui.action(editor, accesskit::Action::Focus);
        ui.frame(vec![Event::Paste(
            "11=TAG-ORDER\n55=WIDGET\n54=1\n38=40\n40=2\n44=19.50\n18=6\n59=1\n22=8\n48=SEC-DEMO\n60=20260101-00:00:00.000\n453=1\n448=DEMO-PARTY\n447=D\n452=1".into(),
        )], Modifiers::NONE);
    } else {
        ui.key(Key::N, Modifiers::CTRL);
        ui.enter("Symbol", "WIDGET");
        ui.enter("Quantity", "40");
        ui.enter("Limit price", "19.50");
        ui.enter_additional_tags("9003=Guided limit");
    }
    ui.click("Send");
    let raw = peer.receive(b"D", 2);
    ui.wait_for_workspace();
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    let original = fields.get(11).unwrap().to_vec();
    assert_eq!(fields.get(55), Some(&b"WIDGET"[..]));
    assert_eq!(fields.get(44), Some(&b"19.50"[..]));
    if !from_tags {
        assert_eq!(fields.get(9003), Some(b"Guided limit".as_slice()));
    }
    peer.send(
        b"8",
        2,
        &[
            (11, &original),
            (55, b"WIDGET"),
            (39, b"0"),
            (150, b"0"),
            (58, b"Training acknowledgement"),
        ],
    );
    ui.click("ExecutionReport");
    ui.wait_for(accesskit::Role::Label, "Training acknowledgement");
    ui.click("Raw");
    ui.wait_for(
        accesskit::Role::Label,
        "Export keeps the original byte values.",
    );
    ui.click("Fields");

    ui.click("NewOrderSingle");
    ui.key(Key::R, Modifiers::CTRL);
    ui.assert_selected_reference(&original);
    ui.enter("Limit price", "19.75");
    ui.enter_additional_tags("9003=Guided replace");
    ui.click("Send");
    let raw = peer.receive(b"G", 3);
    ui.wait_for_workspace();
    let fields = decoder.decode(&raw).unwrap().fields();
    assert_eq!(fields.get(41), Some(original.as_slice()));
    assert_eq!(fields.get(38), Some(&b"40"[..]));
    assert_eq!(fields.get(44), Some(&b"19.75"[..]));
    assert_eq!(fields.get(9003), Some(b"Guided replace".as_slice()));
    if from_tags {
        assert_eq!(fields.get(18), Some(&b"6"[..]));
        assert_eq!(fields.get(59), Some(&b"1"[..]));
        assert_eq!(fields.get(48), Some(&b"SEC-DEMO"[..]));
        assert_eq!(fields.get(22), Some(&b"8"[..]));
        let parties = Version::Fix44.dictionary().group("G", 453).unwrap();
        let group = fields.group(parties).unwrap().unwrap();
        assert_eq!(
            group.entries().next().unwrap().get(448),
            Some(&b"DEMO-PARTY"[..])
        );
        assert_ne!(fields.get(60), Some(&b"20260101-00:00:00.000"[..]));
        for header in [35, 34, 49, 52, 56] {
            assert_eq!(fields.iter().filter(|(tag, _)| *tag == header).count(), 1);
        }
    }
    let replace = fields.get(11).unwrap().to_vec();

    ui.click("OrderCancelReplaceRequest");
    ui.key(Key::K, Modifiers::CTRL);
    ui.assert_selected_reference(&replace);
    ui.enter_additional_tags("9003=Guided cancel");
    ui.click("Send");
    let raw = peer.receive(b"F", 4);
    ui.wait_for_workspace();
    let fields = decoder.decode(&raw).unwrap().fields();
    assert_eq!(fields.get(41), Some(replace.as_slice()));
    assert_eq!(fields.get(9003), Some(b"Guided cancel".as_slice()));
    if from_tags {
        manual_replace(&mut ui, &mut peer, &replace);
    }
}

fn manual_replace(ui: &mut Driver, peer: &mut Wire, previous: &[u8]) {
    ui.click("OrderCancelReplaceRequest");
    ui.key(Key::R, Modifiers::CTRL);
    ui.assert_selected_reference(previous);
    let manual = ui.wait_for(accesskit::Role::Link, "Enter original details manually");
    ui.action(manual, accesskit::Action::Click);
    for label in ["OrigClOrdID", "Symbol", "Quantity", "Limit price"] {
        let id = ui.wait_for(accesskit::Role::TextInput, label);
        let input = ui.nodes.iter().find(|(node, _)| *node == id).unwrap();
        assert_eq!(input.1.value().unwrap_or_default(), "");
    }
    assert!(
        ui.find(accesskit::Role::Label, "OrigClOrdID (read-only)")
            .is_none()
    );
    ui.enter("OrigClOrdID", "MANUAL-ORDER");
    ui.enter("Symbol", "MANUAL");
    ui.enter("Quantity", "7");
    ui.enter("Limit price", "6.20");
    ui.click("Send");

    let raw = peer.receive(b"G", 5);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    assert_eq!(fields.get(41), Some(&b"MANUAL-ORDER"[..]));
    assert_eq!(fields.get(55), Some(&b"MANUAL"[..]));
    for carried in [18, 22, 48, 59, 453] {
        assert_eq!(fields.get(carried), None);
    }
}

fn choose_status(ui: &mut Driver, status: &str) {
    let dropdown = ui.wait_for(accesskit::Role::ComboBox, "OrdStatus");
    ui.action(dropdown, accesskit::Action::Click);
    ui.click(status);
}

fn right_click_row(ui: &mut Driver, label: &str) {
    let id = ui.wait_for(accesskit::Role::Button, label);
    let rect = ui
        .nodes
        .iter()
        .find(|(node, _)| *node == id)
        .unwrap()
        .1
        .bounds()
        .unwrap();
    #[allow(clippy::cast_possible_truncation)]
    let pos = egui::pos2(
        f32::midpoint(rect.x0 as f32, rect.x1 as f32),
        f32::midpoint(rect.y0 as f32, rect.y1 as f32),
    );
    ui.frame(vec![Event::PointerMoved(pos)], Modifiers::NONE);
    for pressed in [true, false] {
        ui.frame(
            vec![Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
        );
    }
}

#[test]
fn execution_context_menu_sends_reports_and_uses_sent_trade_reference() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ui, mut peer) = connected_workspace(&dir.path().join("report.db"));
    peer.send(
        b"D",
        2,
        &[
            (11, b"ORDER-42"),
            (55, b"WIDGET"),
            (54, b"1"),
            (38, b"10"),
            (40, b"2"),
            (44, b"19.50"),
        ],
    );
    right_click_row(&mut ui, "NewOrderSingle");
    ui.click("Acknowledge");
    ui.wait_for(accesskit::Role::TextInput, "OrderID");
    ui.click("Send");

    let mut decoder = Decoder::new(256);
    let raw = peer.receive(b"8", 2);
    let ack = decoder.decode(&raw).unwrap().fields();
    let order_id = ack.get(37).unwrap().to_vec();
    let ack_id = ack.get(17).unwrap().to_vec();
    assert_eq!(ack.get(11), Some(b"ORDER-42".as_slice()));
    assert_eq!(ack.get(150), Some(b"0".as_slice()));
    assert_eq!(ack.get(39), Some(b"0".as_slice()));
    assert_eq!(ack.get(151), Some(b"10".as_slice()));
    assert_eq!(ack.get(32), None);
    ui.wait_for_workspace();

    ui.click("Execution actions");
    ui.click("Partial fill");
    ui.click("Send");
    ui.wait_for(accesskit::Role::Label, "Enter CumQty");
    ui.enter("CumQty", "2");
    ui.enter("LeavesQty", "8");
    ui.enter("AvgPx", "19.25");
    ui.enter("LastQty", "2");
    ui.enter("LastPx", "19.25");
    ui.enter_additional_tags("35=8");
    ui.click("Send");
    ui.wait_for(accesskit::Role::Label, "Tag 35 is reserved");
    ui.enter(
        "Tag=Value pairs separated by semicolons",
        " 29=2 ; 58=Test fill=2; ",
    );
    ui.click("Send");

    let raw = peer.receive(b"8", 3);
    let trade = decoder.decode(&raw).unwrap().fields();
    let trade_id = trade.get(17).unwrap().to_vec();
    assert_ne!(ack_id, trade_id);
    assert!(trade_id.starts_with(b"FT-"));
    for (tag, value) in [
        (37, order_id.as_slice()),
        (150, b"F"),
        (39, b"1"),
        (32, b"2"),
        (31, b"19.25"),
        (29, b"2"),
        (58, b"Test fill=2"),
    ] {
        assert_eq!(trade.get(tag), Some(value));
    }
    ui.wait_for_workspace();

    ui.click("ExecutionReport");
    ui.click("Execution actions");
    ui.click("Trade bust");
    choose_status(&mut ui, "0 · New");
    ui.enter("CumQty", "0");
    ui.enter("LeavesQty", "10");
    ui.enter("AvgPx", "0");
    ui.click("Send");
    let raw = peer.receive(b"8", 4);
    let bust = decoder.decode(&raw).unwrap().fields();
    assert_eq!(bust.get(19), Some(trade_id.as_slice()));
    assert_eq!(bust.get(37), Some(order_id.as_slice()));
    assert_eq!(bust.get(150), Some(b"H".as_slice()));
    assert_eq!(bust.get(39), Some(b"0".as_slice()));
    assert_eq!(bust.get(32), None);
}

#[test]
fn execution_actions_use_the_selected_list_child() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ui, mut peer) = connected_workspace(&dir.path().join("list-report.db"));
    peer.send(
        b"E",
        2,
        &[
            (66, b"LIST-9"),
            (394, b"1"),
            (68, b"2"),
            (73, b"2"),
            (11, b"CHILD-FIRST"),
            (67, b"1"),
            (55, b"ALPHA"),
            (54, b"1"),
            (38, b"10"),
            (40, b"2"),
            (44, b"3.50"),
            (11, b"CHILD-SECOND"),
            (67, b"2"),
            (55, b"BETA"),
            (54, b"2"),
            (38, b"20"),
            (40, b"2"),
            (44, b"4.25"),
            (59, b"6"),
            (126, b"20271120-15:00:00"),
        ],
    );
    ui.click("NewOrderList");
    ui.click("Execution actions");
    ui.wait_for(accesskit::Role::Label, "Select a list order");
    assert!(ui.find(accesskit::Role::Button, "Acknowledge").is_none());
    ui.click("CHILD-SECOND");
    ui.click("Acknowledge");
    ui.click("Send");
    let raw = peer.receive(b"8", 2);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    for (tag, value) in [
        (66, b"LIST-9".as_slice()),
        (11, b"CHILD-SECOND"),
        (55, b"BETA"),
        (54, b"2"),
        (38, b"20"),
        (44, b"4.25"),
        (151, b"20"),
        (59, b"6"),
        (126, b"20271120-15:00:00"),
    ] {
        assert_eq!(fields.get(tag), Some(value));
    }
    assert_eq!(fields.get(73), None);
}

#[test]
fn multileg_execution_and_replace_reject_keep_references() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ui, mut peer) = connected_workspace(&dir.path().join("multileg-report.db"));
    let legs = [
        (555, b"2".as_slice()),
        (600, b"FUT-Z"),
        (602, b"DEC"),
        (603, b"8"),
        (604, b"1"),
        (605, b"ALT-Z"),
        (606, b"8"),
        (610, b"202712"),
        (623, b"1"),
        (624, b"1"),
        (600, b"FUT-H"),
        (610, b"202803"),
        (623, b"2"),
        (624, b"2"),
    ];
    let mut order = vec![
        (11, b"SPREAD".as_slice()),
        (55, b"STRATEGY"),
        (54, b"B"),
        (38, b"3"),
        (40, b"2"),
        (44, b"-0.25"),
    ];
    order.extend_from_slice(&legs[..8]);
    order.extend([(618, b"4".as_slice()), (619, b"TEST".as_slice())]);
    order.extend_from_slice(&legs[8..]);
    peer.send(b"AB", 2, &order);
    ui.click("NewOrderMultileg");
    ui.click("Execution actions");
    ui.click("Acknowledge");
    ui.click("Send");
    let raw = peer.receive(b"8", 2);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    let order_id = fields.get(37).unwrap().to_vec();
    assert_eq!(fields.get(11), Some(b"SPREAD".as_slice()));
    assert_eq!(fields.get(442), Some(b"3".as_slice()));
    assert_eq!(fields.get(44), Some(b"-0.25".as_slice()));
    let definition = Version::Fix44.dictionary().group("8", 555).unwrap();
    let group = fields.group(definition).unwrap().unwrap();
    let entries = group.entries().collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    for tag in [618, 619, 621, 622] {
        assert_eq!(fields.get(tag), None);
    }
    assert_eq!(entries[0].iter().collect::<Vec<_>>(), legs[1..10]);
    assert_eq!(entries[1].iter().collect::<Vec<_>>(), legs[10..]);
    ui.wait_for_workspace();

    let mut replace = vec![
        (11, b"CHANGE".as_slice()),
        (41, b"SPREAD"),
        (37, &order_id),
        (55, b"STRATEGY"),
        (54, b"B"),
        (38, b"3"),
        (40, b"2"),
        (44, b"-0.15"),
    ];
    replace.extend(legs);
    peer.send(b"AC", 3, &replace);
    ui.click("MultilegOrderCancelReplace");
    ui.click("Execution actions");
    assert!(ui.find(accesskit::Role::Button, "Reject order").is_none());
    ui.click("Reject replace request");
    choose_status(&mut ui, "0 · New");
    ui.enter_additional_tags("58=Replace refused");
    ui.click("Send");
    let raw = peer.receive(b"9", 3);
    let fields = decoder.decode(&raw).unwrap().fields();
    for (tag, value) in [
        (37, order_id.as_slice()),
        (11, b"CHANGE"),
        (41, b"SPREAD"),
        (434, b"2"),
        (39, b"0"),
        (58, b"Replace refused"),
    ] {
        assert_eq!(fields.get(tag), Some(value));
    }
    assert_eq!(fields.get(150), None);
    assert_eq!(fields.get(555), None);
}

#[test]
fn execution_status_echoes_the_status_request_id() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ui, mut peer) = connected_workspace(&dir.path().join("status-report.db"));
    peer.send(
        b"H",
        2,
        &[
            (11, b"ORDER-7"),
            (37, b"VENUE-7"),
            (55, b"XYZ"),
            (54, b"1"),
            (790, b"QUERY-9"),
        ],
    );
    ui.click("OrderStatusRequest");
    ui.click("Execution actions");
    assert!(ui.find(accesskit::Role::Button, "Acknowledge").is_none());
    ui.click("Order status");
    choose_status(&mut ui, "1 · PartiallyFilled");
    ui.enter("OrderQty", "12");
    ui.enter("CumQty", "3");
    ui.enter("LeavesQty", "9");
    ui.enter("AvgPx", "5.25");
    ui.click("Send");

    let raw = peer.receive(b"8", 2);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    for (tag, value) in [
        (11, b"ORDER-7".as_slice()),
        (37, b"VENUE-7"),
        (790, b"QUERY-9"),
        (17, b"0"),
        (39, b"1"),
        (150, b"I"),
    ] {
        assert_eq!(fields.get(tag), Some(value));
    }
    assert_eq!(fields.get(20), None);
    assert_eq!(fields.get(31), None);
}

#[test]
fn fix42_correction_and_bust_use_resulting_fill_type_and_exec_ref() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ui, mut peer) =
        connected_workspace_version(&dir.path().join("fix42-correction.db"), Version::Fix42);
    peer.send(
        b"D",
        2,
        &[(11, b"ORDER"), (55, b"XYZ"), (54, b"1"), (38, b"10")],
    );
    ui.click("OrderSingle");
    ui.click("Execution actions");
    ui.click("Full fill");
    ui.enter("CumQty", "9.99999999999999999");
    ui.enter("AvgPx", "109");
    ui.enter("LastShares", "9");
    ui.enter("LastPx", "110");
    ui.click("Send");
    ui.wait_for(
        accesskit::Role::Label,
        "A full fill requires CumQty to equal OrderQty.",
    );
    ui.enter("CumQty", "10.000");
    ui.click("Send");
    let raw = peer.receive(b"8", 2);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    let full_id = fields.get(17).unwrap().to_vec();
    assert_eq!(fields.get(150), Some(b"2".as_slice()));
    assert_eq!(fields.get(14), Some(b"10.000".as_slice()));
    assert_eq!(fields.get(151), Some(b"0".as_slice()));
    ui.wait_for_workspace();

    ui.click("ExecutionReport");
    ui.click("Execution actions");
    ui.click("Trade correction");
    choose_status(&mut ui, "1 · PartiallyFilled");
    ui.enter("CumQty", "9");
    ui.enter("LeavesQty", "1");
    ui.enter("AvgPx", "100");
    ui.enter("LastShares", "9");
    ui.enter("LastPx", "100");
    ui.click("Send");

    let raw = peer.receive(b"8", 3);
    let fields = decoder.decode(&raw).unwrap().fields();
    for (tag, value) in [
        (20, b"2".as_slice()),
        (150, b"1"),
        (39, b"1"),
        (19, full_id.as_slice()),
        (14, b"9"),
        (151, b"1"),
    ] {
        assert_eq!(fields.get(tag), Some(value));
    }
    let correction_id = fields.get(17).unwrap().to_vec();
    ui.wait_for_workspace();
    ui.click("ExecutionReport");
    ui.click("Execution actions");
    ui.click("Trade bust");
    choose_status(&mut ui, "4 · Canceled");
    ui.enter("CumQty", "0");
    ui.enter("LeavesQty", "0");
    ui.enter("AvgPx", "0");
    ui.click("Send");
    let raw = peer.receive(b"8", 4);
    let fields = decoder.decode(&raw).unwrap().fields();
    for (tag, value) in [
        (20, b"1".as_slice()),
        (150, b"1"),
        (39, b"4"),
        (19, correction_id.as_slice()),
        (31, b"0"),
        (32, b"0"),
    ] {
        assert_eq!(fields.get(tag), Some(value));
    }
}

#[test]
fn cancel_reject_echoes_the_list_id_on_wire() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ui, mut peer) = connected_workspace(&dir.path().join("list-reject.db"));
    peer.send(
        b"F",
        2,
        &[
            (37, b"VENUE"),
            (11, b"CANCEL"),
            (41, b"CHILD"),
            (66, b"LIST-9"),
        ],
    );
    ui.click("OrderCancelRequest");
    ui.click("Execution actions");
    ui.click("Reject cancel request");
    choose_status(&mut ui, "0 · New");
    ui.click("Send");

    let raw = peer.receive(b"9", 2);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    for (tag, value) in [
        (37, b"VENUE".as_slice()),
        (11, b"CANCEL"),
        (41, b"CHILD"),
        (66, b"LIST-9"),
        (434, b"1"),
    ] {
        assert_eq!(fields.get(tag), Some(value));
    }
}
