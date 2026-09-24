use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Device {
    dir: TempDir,
    child: Option<Child>,
}

impl Device {
    fn new(name: &str) -> Self {
        let device = Self {
            dir: tempfile::tempdir().unwrap(),
            child: None,
        };
        device.ok(&["node", "init", "--name", name]);
        device
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spirit"))
            .arg("--node-dir")
            .arg(self.dir.path())
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn start(&mut self) {
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_spirit"))
                .arg("--node-dir")
                .arg(self.dir.path())
                .args(["node", "serve", "--local"])
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self
            .run(&["mesh", "status"])
            .stdout
            .windows(b"Node: running".len())
            .any(|w| w == b"Node: running")
        {
            assert!(Instant::now() < deadline, "node did not start");
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "node exited"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn three_devices_enroll_by_ticket_and_ping_by_nickname_without_introducer() {
    let mut phone = Device::new("phone");
    let mut desktop = Device::new("desktop");
    let mut laptop = Device::new("laptop");
    phone.ok(&["mesh", "create", "--name", "personal"]);
    phone.start();
    desktop.start();
    laptop.start();
    let svg_path = desktop.dir.path().join("pairing.svg");
    let qr = desktop.ok(&["node", "pair", "--qr-svg", svg_path.to_str().unwrap()]);
    let svg = std::fs::read_to_string(&svg_path).unwrap();
    assert!(svg.contains("<svg") && svg.contains("<path"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [svg_path, desktop.dir.path().join("control.json")] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }
    assert!(qr.contains('█') || qr.contains('▀') || qr.contains('▄'));
    let ticket = qr.lines().find(|line| line.starts_with("spirit1")).unwrap();
    assert!(phone.ok(&["mesh", "add", ticket]).contains("desktop"));
    let ticket = laptop.ok(&["node", "pair", "--no-qr"]);
    phone.ok(&["mesh", "add", ticket.trim()]);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !desktop.ok(&["mesh", "members"]).contains("laptop") {
        assert!(Instant::now() < deadline, "membership did not converge");
        std::thread::sleep(Duration::from_millis(50));
    }
    phone.stop();
    assert!(desktop
        .ok(&["node", "ping", "laptop"])
        .contains("pong from laptop"));
    assert!(laptop
        .ok(&["node", "ping", "desktop"])
        .contains("pong from desktop"));
    assert!(desktop.ok(&["mesh", "status"]).contains("personal"));
    let id = desktop.ok(&["node", "id"]);
    assert!(desktop
        .ok(&["mesh", "members", "--ids"])
        .contains(id.trim()));
    assert!(!desktop.run(&["node", "ping", "stranger"]).status.success());
    assert!(!desktop.run(&["node", "serve", "--local"]).status.success());
    desktop.stop();
    assert!(desktop.ok(&["mesh", "status"]).contains("stopped"));
    desktop.start();
    assert_eq!(desktop.ok(&["node", "id"]), id);
    assert!(desktop
        .ok(&["node", "ping", "laptop"])
        .contains("pong from laptop"));
    let mut duplicate = Device::new("laptop");
    duplicate.start();
    let ticket = duplicate.ok(&["node", "pair", "--no-qr"]);
    desktop.ok(&["mesh", "add", ticket.trim()]);
    let error = desktop.run(&["node", "ping", "laptop"]);
    assert!(!error.status.success());
    assert!(String::from_utf8_lossy(&error.stderr).contains("ambiguous"));
    let duplicate_id = duplicate.ok(&["node", "id"]);
    assert!(desktop
        .ok(&["node", "ping", duplicate_id.trim()])
        .contains("pong from laptop"));
}

#[test]
fn initialization_defaults_to_hostname_and_persists_identity() {
    let dir = tempfile::tempdir().unwrap();
    let device = Device { dir, child: None };
    let output = device.ok(&["node", "init"]);
    assert!(output.contains(hostname::get().unwrap().to_str().unwrap()));
    let id = device.ok(&["node", "id"]);
    device.ok(&["node", "init"]);
    assert_eq!(device.ok(&["node", "id"]), id);
    assert!(!device.run(&["node", "pair"]).status.success());
    assert!(device.ok(&["mesh", "status"]).contains("not enrolled"));
    assert!(!device.dir.path().join("store").exists());
}

#[test]
fn a_device_leaves_its_mesh_and_is_enrolled_again() {
    let mut desktop = Device::new("desktop");
    let mut laptop = Device::new("laptop");
    desktop.ok(&["mesh", "create", "--name", "personal"]);
    desktop.start();
    laptop.start();
    let ticket = laptop.ok(&["node", "pair", "--no-qr"]);
    desktop.ok(&["mesh", "add", ticket.trim()]);

    assert_eq!(
        laptop.ok(&["mesh", "leave"]),
        "Left personal and notified its 1 remaining member.\n"
    );
    assert!(laptop
        .ok(&["mesh", "status"])
        .contains("Mesh: not enrolled"));
    assert!(!desktop.ok(&["mesh", "members"]).contains("laptop"));
    assert!(!laptop.run(&["mesh", "leave"]).status.success());
    assert!(!desktop.run(&["node", "ping", "laptop"]).status.success());

    let ticket = laptop.ok(&["node", "pair", "--no-qr"]);
    desktop.ok(&["mesh", "add", ticket.trim()]);
    assert!(desktop.ok(&["mesh", "members"]).contains("laptop"));
    assert!(laptop.ok(&["mesh", "status"]).contains("Mesh: personal"));
    assert!(desktop
        .ok(&["node", "ping", "laptop"])
        .starts_with("pong from laptop"));

    laptop.stop();
    desktop.stop();
    assert_eq!(
        desktop.ok(&["mesh", "leave"]),
        "Left personal. Notified 0 of 1 remaining members; notified members relay the departure; the others can also learn it when they next reach this device while it is serving.\n"
    );
}
