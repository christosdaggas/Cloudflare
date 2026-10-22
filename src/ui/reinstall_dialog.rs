//! Installing the connector again on a machine where it was removed.
//!
//! The scenario is a wiped or replaced machine: the tunnel still exists in
//! Cloudflare, but nothing serves it any more. The dialog gathers in one place
//! what used to live in two different tabs — the run-token copy in Remote
//! Configuration and the manual commands in Managed Hosts — and says
//! explicitly what it cannot do: a tunnel deleted in Cloudflare cannot be
//! reinstalled, only recreated.
//!
//! The run token is only ever placed on the clipboard, never rendered into a
//! label or a command. The manual commands read it from stdin, for the same
//! reason the deployment dialog pipes it rather than printing it.

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::domain::tunnel::Tunnel;
use crate::services::hosts;
use crate::ui::state::AppState;
use crate::ui::tunnel_detail_page::copy_tunnel_run_token;
use crate::ui::widgets as w;

/// Opens the reinstall guide for an existing tunnel.
pub fn present(parent: &impl IsA<gtk4::Widget>, state: &AppState, tunnel: &Tunnel) {
    let dialog = adw::Dialog::builder()
        .title("Install the connector again")
        .content_width(600)
        .content_height(640)
        .build();

    let column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(18)
        .margin_top(12)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();

    column.append(&w::inline_message(
        "dialog-information-symbolic",
        &format!(
            "This sets up cloudflared for the existing tunnel “{}” on a machine \
             where it was removed. If the tunnel itself was deleted in Cloudflare, \
             create a new tunnel instead — a deleted tunnel cannot be reinstalled.",
            tunnel.name
        ),
        "dimmed",
    ));

    // ── Automatic ──────────────────────────────────────────────────────
    let auto_group = w::group(
        "Automatic — over SSH",
        "Register the machine under Remote Hosts and deploy the connector from \
         here, exactly as on first setup. Nothing is typed on the machine itself.",
    );

    let open_hosts = gtk4::Button::builder()
        .label("Open Remote Hosts")
        .halign(gtk4::Align::Center)
        .action_name("win.page")
        .action_target(&"hosts".to_variant())
        .css_classes(["pill"])
        .build();
    {
        let dialog = dialog.downgrade();
        open_hosts.connect_clicked(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }
    auto_group.add(&{
        let wrapper = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .margin_top(12)
            .build();
        wrapper.append(&open_hosts);
        wrapper
    });
    column.append(&auto_group);

    // ── Manual ─────────────────────────────────────────────────────────
    let manual_group = w::group(
        "Manual — on the machine itself",
        "Two steps, in this order. The token is copied to the clipboard and \
         pasted into the command below when asked — it is never shown on screen.",
    );

    let copy_row = adw::ActionRow::builder()
        .title("1. Copy the run token")
        .subtitle("Puts the token on the clipboard. Paste it into the command below.")
        .build();
    let copy_button = gtk4::Button::builder()
        .label("Copy")
        .valign(gtk4::Align::Center)
        .css_classes(["suggested-action"])
        .build();
    {
        let state = state.clone();
        let tunnel_id = tunnel.id.clone();
        copy_button.connect_clicked(move |button| {
            copy_tunnel_run_token(button, &state, &tunnel_id);
        });
    }
    copy_row.add_suffix(&copy_button);
    manual_group.add(&copy_row);
    column.append(&manual_group);

    column.append(&w::copyable_code_block(
        &hosts::manual_setup_instructions(&tunnel.id),
        "2. Run these commands on the machine:",
    ));

    column.append(&w::inline_message(
        "dialog-warning-symbolic",
        "Treat the run token like a password — anyone holding it can serve this \
         tunnel. Clear the clipboard afterwards.",
        "warning",
    ));

    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&column)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&scroller));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(parent.as_ref()));
}
