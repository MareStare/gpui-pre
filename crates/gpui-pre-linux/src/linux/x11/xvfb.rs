//! An X server for tests, with fake input of the XTEST extension.

use std::{
    ffi::CString,
    io::{BufRead as _, BufReader},
    process::{Child, Command, Stdio},
};

use x11rb::xcb_ffi::XCBConnection;

/// An Xvfb display. Drop stops it.
pub(super) struct Xvfb {
    server: Child,
    display: CString,
}

impl Xvfb {
    /// Starts a display with a free number. Panics when Xvfb is missing.
    pub(super) fn start() -> Self {
        let mut server = Command::new("Xvfb")
            .args(["-displayfd", "1", "-nolisten", "tcp", "-noreset"])
            .args(["-screen", "0", "1280x1024x24"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("Xvfb: {error}"));
        // Xvfb writes the number of its display when it is ready.
        let mut line = String::new();
        BufReader::new(server.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let number = line.trim();
        assert!(!number.is_empty(), "Xvfb gives no display number");
        Self {
            display: CString::new(format!(":{number}")).unwrap(),
            server,
        }
    }

    /// Connects a client. Returns the connection and the number of its screen.
    pub(super) fn connect(&self) -> (XCBConnection, usize) {
        XCBConnection::connect(Some(&self.display)).unwrap()
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        drop(self.server.kill());
        drop(self.server.wait());
    }
}
