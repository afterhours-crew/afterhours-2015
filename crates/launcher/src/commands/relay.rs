// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `relay` (internal): the stand-in for a server on this computer when the
//! game plays against one elsewhere (`start --remote`). It relays the
//! game's launcher services and redirector to that server
//! ([`crate::relay`]) and prints a readiness record like the server's, so
//! the supervisor starts, routes and watches the game exactly as for a
//! server here. It stops when its stop file appears.

use crate::{cli::RelayArgs, exit::Result, relay::Relay};
use std::{io::Write, time::Duration};

pub fn relay(args: &RelayArgs) -> Result<()> {
    let launcher_services = Relay::start(args.lsx_port, args.launcher_services.into())?;
    let redirector = Relay::start(0, args.redirector.into())?;
    let ready = serde_json::json!({
        "ready": true,
        "pid": std::process::id(),
        "redirector": redirector.address().to_string(),
        "lsx": launcher_services.address().to_string(),
        "relays_to": {
            "redirector": args.redirector.to_string(),
            "launcher_services": args.launcher_services.to_string(),
        },
    });
    let mut out = std::io::stdout().lock();
    writeln!(out, "{ready}")?;
    out.flush()?;
    drop(out);
    while !args.stop_file.exists() {
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}
