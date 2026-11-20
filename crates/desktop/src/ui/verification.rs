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
    let addr = TcpListener::bind("127.0.0.1:0").unwrap();
    let profile = Profile {
        name: "Loopback example".into(),
        version: Version::Fix44,
        role: Role::Initiator,
        sender: "TRAINER".into(),
        target: "VENUE".into(),
        address: addr.local_addr().unwrap(),
        heartbeat: 30,
    };
    let mut ui = Driver::new(path);
    ui.click("Add profile");
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
    ui.click("Send");

    let raw = peer.receive(b"AB", 2);
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    assert_eq!(fields.get_u64(555), Some(2));
    assert_eq!(fields.get(461), Some(b"FMXXXX".as_slice()));
    assert_eq!(fields.get(44), Some(b"-0.25".as_slice()));
    ui.click("Order list");
    ui.enter("Symbol", "ALPHA");
    ui.enter("Quantity", "12");
    ui.enter("Limit price", "19.50");
    ui.click("Send");

    let raw = peer.receive(b"E", 3);
    let fields = decoder.decode(&raw).unwrap().fields();
    assert_eq!(fields.get_u64(73), Some(1));
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
    }
    ui.click("Send");
    let raw = peer.receive(b"D", 2);
    ui.wait_for_workspace();
    let mut decoder = Decoder::new(256);
    let fields = decoder.decode(&raw).unwrap().fields();
    let original = fields.get(11).unwrap().to_vec();
    assert_eq!(fields.get(55), Some(&b"WIDGET"[..]));
    assert_eq!(fields.get(44), Some(&b"19.50"[..]));
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
    ui.click("Send");
    let raw = peer.receive(b"G", 3);
    ui.wait_for_workspace();
    let fields = decoder.decode(&raw).unwrap().fields();
    assert_eq!(fields.get(41), Some(original.as_slice()));
    assert_eq!(fields.get(38), Some(&b"40"[..]));
    assert_eq!(fields.get(44), Some(&b"19.75"[..]));
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
    ui.click("Send");
    let raw = peer.receive(b"F", 4);
    ui.wait_for_workspace();
    assert_eq!(
        decoder.decode(&raw).unwrap().fields().get(41),
        Some(replace.as_slice())
    );
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
