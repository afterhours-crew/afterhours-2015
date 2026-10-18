// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

fn main() {
    println!("cargo:rerun-if-changed=resources/afterhours.rc");
    println!("cargo:rerun-if-changed=resources/afterhours.ico");
    // Non-Windows targets are a no-op. Windows builds must embed the icon;
    // a missing resource compiler must not silently ship an unbranded binary.
    embed_resource::compile_for(
        "resources/afterhours.rc",
        ["Afterhours"],
        embed_resource::NONE,
    )
    .manifest_required()
    .expect("could not embed the Afterhours icon (Windows SDK resource compiler required)");
}
