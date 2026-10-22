//! Adding and editing a remote host, and the host-key trust prompt.
//!
//! The trust prompt is the important part of this file. It is presented as a
//! decision with two outcomes and no third: a fingerprint that has never been
//! seen can be accepted after being read, and a fingerprint that has *changed*
//! cannot be accepted at all. The second case has one button, "Close", because
//! the two explanations for it — the machine was rebuilt, or something is
//! sitting between you and it — cannot be told apart from here, and only one
//! of them is safe.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use secrecy::SecretString;

use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::host::{RemoteAuth, RemoteHost, RemoteHostDraft};
use crate::services::hosts::{self, TrustDecision};
use crate::services::runtime;
use crate::storage;
use crate::ui::state::AppState;
use crate::ui::widgets as w;

struct Form {
    name: adw::EntryRow,
    address: adw::EntryRow,
    port: adw::SpinRow,
    username: adw::EntryRow,
    auth: adw::ComboRow,
    key_path: adw::ActionRow,
    key_path_value: RefCell<Option<std::path::PathBuf>>,
    passphrase: adw::PasswordEntryRow,
    unit: adw::EntryRow,
    tunnel: adw::ComboRow,
    tunnel_ids: Vec<Option<String>>,
    notes: adw::EntryRow,
    error: gtk4::Label,
    save: gtk4::Button,
}

impl Form {
    fn uses_key_file(&self) -> bool {
        self.auth.selected() == 1
    }

    fn draft(&self) -> Result<RemoteHostDraft, crate::domain::validation::ValidationError> {
        let auth = if self.uses_key_file() {
            let path = self.key_path_value.borrow().clone().ok_or_else(|| {
                crate::domain::validation::ValidationError::new(
                    "Choose the private key file to use.",
                )
            })?;
            RemoteAuth::KeyFile {
                path,
                has_passphrase: !self.passphrase.text().is_empty(),
            }
        } else {
            RemoteAuth::Agent
        };

        hosts::validate_draft(&RemoteHostDraft {
            name: self.name.text().to_string(),
            address: self.address.text().to_string(),
            port: self.port.value() as u16,
            username: self.username.text().to_string(),
            auth,
            service_unit: self.unit.text().to_string(),
            tunnel_id: self
                .tunnel_ids
                .get(self.tunnel.selected() as usize)
                .cloned()
                .flatten(),
            notes: Some(self.notes.text().to_string()),
        })
    }

    fn revalidate(&self) {
        let key_file = self.uses_key_file();
        self.key_path.set_visible(key_file);
        self.passphrase.set_visible(key_file);

        match self.draft() {
            Ok(_) => {
                self.error.set_visible(false);
                self.save.set_sensitive(true);
            }
            Err(error) => {
                // Only complain once the user has typed something; an empty
                // form is not an error, it is an unfinished one.
                let started = !self.name.text().is_empty() || !self.address.text().is_empty();
                self.error.set_label(error.message());
                self.error.set_visible(started);
                self.save.set_sensitive(false);
            }
        }
    }
}

pub fn present_new(parent: &impl IsA<gtk4::Widget>, state: &AppState) {
    present(parent, state, None);
}

pub fn present_edit(parent: &impl IsA<gtk4::Widget>, state: &AppState, host: &RemoteHost) {
    present(parent, state, Some(host.clone()));
}

fn present(parent: &impl IsA<gtk4::Widget>, state: &AppState, editing: Option<RemoteHost>) {
    let settings = state.settings();

    let dialog = adw::Dialog::builder()
        .title(if editing.is_some() {
            "Edit host"
        } else {
            "Add a remote host"
        })
        .content_width(560)
        .content_height(680)
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
        "channel-secure-symbolic",
        "Nothing connects to this host until you have been shown its key fingerprint \
         and accepted it. Password authentication is not supported.",
        "dimmed",
    ));

    // ── Connection ───────────────────────────────────────────────────────
    let connection = w::group("Connection", "");

    let name = adw::EntryRow::builder().title("Name").build();
    let address = adw::EntryRow::builder()
        .title("Hostname or IP address")
        .build();
    let port = adw::SpinRow::with_range(1.0, 65535.0, 1.0);
    port.set_title("SSH port");
    port.set_value(f64::from(settings.default_ssh_port));
    let username = adw::EntryRow::builder().title("Username").build();
    username.set_text(&settings.default_ssh_username);

    connection.add(&name);
    connection.add(&address);
    connection.add(&port);
    connection.add(&username);
    column.append(&connection);

    // ── Authentication ───────────────────────────────────────────────────
    let auth_group = w::group(
        "Authentication",
        "ssh-agent is preferred: nothing is read from disk and no passphrase has to be \
         stored anywhere.",
    );

    let methods = gtk4::StringList::new(&["ssh-agent", "Private key file"]);
    let auth = adw::ComboRow::builder()
        .title("Method")
        .model(&methods)
        .build();
    auth_group.add(&auth);

    let key_path = adw::ActionRow::builder()
        .title("Private key")
        .subtitle("No file chosen")
        .visible(false)
        .build();
    let choose = gtk4::Button::builder()
        .label("Choose…")
        .valign(gtk4::Align::Center)
        .build();
    key_path.add_suffix(&choose);
    auth_group.add(&key_path);

    let passphrase = adw::PasswordEntryRow::builder()
        .title("Key passphrase (optional)")
        .visible(false)
        .build();
    auth_group.add(&passphrase);

    let note = adw::ActionRow::builder()
        .title("The key file is never copied")
        .subtitle(
            "Only its path is saved. A passphrase, if you give one, goes to your \
             desktop's credential store — never to this application's database.",
        )
        .build();
    note.add_prefix(&gtk4::Image::from_icon_name("dialog-information-symbolic"));
    auth_group.add(&note);
    column.append(&auth_group);

    // ── Service ──────────────────────────────────────────────────────────
    let service_group = w::group(
        "cloudflared service",
        "Which systemd unit on that host this application may control. Only `.service` \
         units are accepted.",
    );

    let unit = adw::EntryRow::builder().title("Service unit").build();
    unit.set_text(&settings.default_service_unit);
    service_group.add(&unit);

    let tunnels = state.tunnels();
    let tunnel_names = gtk4::StringList::new(&["Not associated"]);
    let mut tunnel_ids: Vec<Option<String>> = vec![None];
    for row in &tunnels {
        tunnel_names.append(&row.tunnel.name);
        tunnel_ids.push(Some(row.tunnel.id.clone()));
    }
    let tunnel = adw::ComboRow::builder()
        .title("Tunnel")
        .subtitle("Which tunnel this host is expected to serve")
        .model(&tunnel_names)
        .build();
    service_group.add(&tunnel);

    let notes = adw::EntryRow::builder().title("Note (optional)").build();
    service_group.add(&notes);
    column.append(&service_group);

    let error = gtk4::Label::builder()
        .halign(gtk4::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    error.add_css_class("error");
    error.add_css_class("caption");
    column.append(&error);

    let save = gtk4::Button::builder()
        .label(if editing.is_some() {
            "Save"
        } else {
            "Add Host"
        })
        .halign(gtk4::Align::Center)
        .sensitive(false)
        .css_classes(["pill", "suggested-action"])
        .build();
    column.append(&save);

    // Pre-fill when editing.
    if let Some(host) = &editing {
        name.set_text(&host.name);
        address.set_text(&host.address);
        port.set_value(f64::from(host.port));
        username.set_text(&host.username);
        unit.set_text(&host.service_unit);
        if let Some(notes_text) = &host.notes {
            notes.set_text(notes_text);
        }
        if let Some(index) = tunnel_ids
            .iter()
            .position(|id| id.as_deref() == host.tunnel_id.as_deref())
        {
            tunnel.set_selected(index as u32);
        }
        if let RemoteAuth::KeyFile { path, .. } = &host.auth {
            auth.set_selected(1);
            key_path.set_subtitle(&path.to_string_lossy());
        }
    }

    let form = Rc::new(Form {
        name: name.clone(),
        address: address.clone(),
        port: port.clone(),
        username: username.clone(),
        auth: auth.clone(),
        key_path: key_path.clone(),
        key_path_value: RefCell::new(match &editing {
            Some(RemoteHost {
                auth: RemoteAuth::KeyFile { path, .. },
                ..
            }) => Some(path.clone()),
            _ => None,
        }),
        passphrase: passphrase.clone(),
        unit: unit.clone(),
        tunnel: tunnel.clone(),
        tunnel_ids,
        notes: notes.clone(),
        error,
        save: save.clone(),
    });

    {
        let form = Rc::clone(&form);
        let revalidate = move || form.revalidate();

        {
            let revalidate = revalidate.clone();
            auth.connect_selected_notify(move |_| revalidate());
        }
        {
            let revalidate = revalidate.clone();
            tunnel.connect_selected_notify(move |_| revalidate());
        }
        {
            let revalidate = revalidate.clone();
            port.connect_value_notify(move |_| revalidate());
        }
        for entry in [&name, &address, &username, &unit, &notes] {
            let revalidate = revalidate.clone();
            entry.connect_changed(move |_| revalidate());
        }
        {
            let revalidate = revalidate.clone();
            passphrase.connect_changed(move |_| revalidate());
        }
        revalidate();
    }

    {
        let form = Rc::clone(&form);
        let dialog_ref = dialog.clone();
        choose.connect_clicked(move |button| {
            let Some(window) = button.root().and_downcast::<gtk4::Window>() else {
                return;
            };
            let chooser = gtk4::FileDialog::builder()
                .title("Choose a private key")
                .modal(true)
                .build();

            let form = Rc::clone(&form);
            let _ = &dialog_ref;
            // Cancelling the chooser arrives as an error and is not worth
            // reporting, so only the success case is handled.
            glib::spawn_future_local(async move {
                if let Ok(file) = chooser.open_future(Some(&window)).await {
                    if let Some(path) = file.path() {
                        form.key_path.set_subtitle(&path.to_string_lossy());
                        *form.key_path_value.borrow_mut() = Some(path);
                        form.revalidate();
                    }
                }
            });
        });
    }

    {
        let form = Rc::clone(&form);
        let state = state.clone();
        let dialog = dialog.clone();
        let parent = parent.as_ref().clone();
        let editing = editing.clone();

        save.connect_clicked(move |button| {
            let Ok(draft) = form.draft() else {
                return;
            };
            button.set_sensitive(false);

            let passphrase = form.passphrase.text().to_string();
            let store = state.store();
            let state_clone = state.clone();
            let parent = parent.clone();
            let dialog = dialog.clone();
            let editing = editing.clone();

            runtime::spawn_blocking(
                move || {
                    store.with(|connection| match &editing {
                        Some(existing) => {
                            let updated = RemoteHost {
                                id: existing.id,
                                name: draft.name.clone(),
                                address: draft.address.clone(),
                                port: draft.port,
                                username: draft.username.clone(),
                                auth: draft.auth.clone(),
                                // Editing a host never re-trusts its key. If
                                // the address changed, the pinned fingerprint
                                // is dropped so the next connection asks
                                // again rather than trusting the old key for
                                // a new machine.
                                host_key_fingerprint: if existing.address == draft.address
                                    && existing.port == draft.port
                                {
                                    existing.host_key_fingerprint.clone()
                                } else {
                                    None
                                },
                                service_unit: draft.service_unit.clone(),
                                tunnel_id: draft.tunnel_id.clone(),
                                created_at: existing.created_at,
                                last_seen_at: existing.last_seen_at,
                                notes: draft.notes.clone(),
                            };
                            let _ = storage::hosts::update(connection, &updated);
                            updated.id
                        }
                        None => storage::hosts::insert(connection, &draft).unwrap_or(0),
                    })
                },
                move |host_id| {
                    let Some(host_id) = host_id else {
                        return;
                    };

                    if !passphrase.is_empty() {
                        let secret = SecretString::from(passphrase);
                        let state_for_secret = state_clone.clone();
                        runtime::spawn_blocking(
                            move || storage::secrets::save_host_passphrase(host_id, &secret),
                            move |result| {
                                if let Err(error) = result {
                                    state_for_secret.record(ActivityEvent::warning(
                                        ActivityCategory::App,
                                        error.user_message(),
                                    ));
                                }
                            },
                        );
                    }

                    state_clone.record(ActivityEvent::info(
                        ActivityCategory::App,
                        "Saved a remote host profile",
                    ));

                    dialog.close();
                    crate::ui::hosts_page::refresh_hosts(&state_clone);

                    // Go straight to the fingerprint prompt: a host that has
                    // not been trusted can do nothing, so leaving the user to
                    // find the button would be a dead end.
                    let state_after = state_clone.clone();
                    let parent = parent.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(200),
                        move || {
                            if let Some(host) = state_after.host(host_id) {
                                if !host.is_trusted() {
                                    verify_host_key(&parent, &state_after, &host);
                                }
                            }
                        },
                    );
                },
            );
        });
    }

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

/// Reads the host's key and asks the user to decide about it.
pub fn verify_host_key(parent: &impl IsA<gtk4::Widget>, state: &AppState, host: &RemoteHost) {
    let controller = state.controller();
    let host_clone = host.clone();
    let host_for_decision = host.clone();
    let parent = parent.as_ref().clone();
    let state = state.clone();

    runtime::spawn(
        async move { hosts::probe_host_key(&controller, &host_clone).await },
        move |result| match result {
            Ok(key) => {
                let decision = TrustDecision::evaluate(&host_for_decision, key);
                state.record(decision.activity(&host_for_decision));
                present_trust_decision(&parent, &state, &host_for_decision, decision);
            }
            Err(error) => {
                state.record(ActivityEvent::error(
                    ActivityCategory::Ssh,
                    error.user_message(),
                ));
                w::inform(&parent, error.title(), &error.user_message());
            }
        },
    );
}

fn present_trust_decision(
    parent: &gtk4::Widget,
    state: &AppState,
    host: &RemoteHost,
    decision: TrustDecision,
) {
    match decision {
        TrustDecision::Unchanged => {
            w::toast(parent, "The host key matches the one you trusted.");
        }

        TrustDecision::FirstContact { key } => {
            let dialog = adw::AlertDialog::new(
                Some("Trust this host?"),
                Some(&format!(
                    "This is the first connection to {}. Check the fingerprint below \
                     against what the server reports before accepting it.",
                    host.display_target()
                )),
            );

            let column = gtk4::Box::builder()
                .orientation(gtk4::Orientation::Vertical)
                .spacing(12)
                .build();

            let group = w::group("", "");
            group.add(&w::property_row("Key type", &key.algorithm));
            group.add(&w::copyable_row("Fingerprint", &key.fingerprint));
            column.append(&group);

            column.append(&w::copyable_code_block(
                &format!(
                    "ssh-keyscan -p {} {} | ssh-keygen -lf -",
                    host.port, host.address
                ),
                "Run this on a machine you trust and compare the output:",
            ));

            dialog.set_extra_child(Some(&column));
            dialog.set_prefer_wide_layout(true);
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("trust", "Trust This Host");
            dialog.set_response_appearance("trust", adw::ResponseAppearance::Suggested);
            // Escape and Enter both decline: trusting a key is a decision, not
            // a default.
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let state = state.clone();
            let host = host.clone();
            // A separate clone for the closure: `parent` is still needed below
            // to present the dialog.
            let parent_for_response = parent.clone();
            dialog.connect_response(None, move |_, response| {
                if response != "trust" {
                    return;
                }

                let store = state.store();
                let host_id = host.id;
                let fingerprint = key.fingerprint.clone();
                let state_clone = state.clone();
                let parent = parent_for_response.clone();
                let host = host.clone();

                runtime::spawn_blocking(
                    move || {
                        store.with(|connection| {
                            storage::hosts::pin_host_key(connection, host_id, &fingerprint)
                        })
                    },
                    move |_| {
                        state_clone.record(
                            ActivityEvent::success(
                                ActivityCategory::Ssh,
                                format!("Trusted the host key for {}", host.name),
                            )
                            .with_target(host.name.clone()),
                        );
                        crate::ui::hosts_page::refresh_hosts(&state_clone);
                        w::toast(
                            &parent,
                            "Host key trusted. You can now test the connection.",
                        );
                    },
                );
            });

            dialog.present(Some(parent));
        }

        TrustDecision::Changed { expected, key } => {
            let dialog = adw::AlertDialog::new(
                Some("Host key changed"),
                Some(
                    "The key this host presented is not the one that was trusted. \
                     Nothing was sent and no credential was offered.\n\nThis happens \
                     when a machine is rebuilt — and it also happens when something is \
                     intercepting the connection. There is no way to tell which from \
                     here, so this application will not connect.",
                ),
            );

            let group = w::group("", "");
            group.add(&w::copyable_row("Trusted fingerprint", &expected));
            group.add(&w::copyable_row("Presented fingerprint", &key.fingerprint));
            group.add(&w::property_row("Key type", &key.algorithm));

            let advice = adw::ActionRow::builder()
                .title("If you rebuilt this machine")
                .subtitle(
                    "Remove the host from Remote Hosts and add it again. That is \
                     deliberately more work than clicking through a warning.",
                )
                .build();
            advice.add_prefix(&gtk4::Image::from_icon_name("dialog-information-symbolic"));
            group.add(&advice);

            let wrapper = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            wrapper.append(&group);
            dialog.set_extra_child(Some(&wrapper));
            dialog.set_prefer_wide_layout(true);

            // One response. There is deliberately no "trust anyway".
            dialog.add_response("close", "Close");
            dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("close"));
            dialog.set_close_response("close");
            dialog.present(Some(parent));
        }
    }
}

use gtk4::glib;
