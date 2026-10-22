//! The Create Tunnel wizard.
//!
//! Five steps, of which only the first is required. That shape is the whole
//! point: a tunnel with no hostname and no connector is a perfectly valid
//! thing to create, and the wizard must not imply otherwise by making the user
//! click through screens they have nothing to say to.
//!
//! Step 3 shows the exact changes before any of them happen, and step 5
//! reports what actually did — per step, with the recovery for whichever one
//! failed. Between them they mean the user is never surprised by what the
//! application did to their account, in either direction.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::cloudflare::Cancel;
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::hostname::{OriginScheme, OriginService, TargetKind};
use crate::domain::validation;
use crate::domain::Zone;
use crate::services::runtime;
use crate::services::tunnels::{self, CreateOutcome, CreateRequest, HostnameRequest};
use crate::ui::state::AppState;
use crate::ui::widgets as w;

const STEPS: &[&str] = &["Tunnel", "Hostname", "Review", "Deploy", "Result"];

struct Wizard {
    state: AppState,
    dialog: adw::Dialog,
    stack: gtk4::Stack,
    step: Cell<usize>,

    heading: gtk4::Label,
    progress: gtk4::Label,
    back: gtk4::Button,
    next: gtk4::Button,

    // Step 1
    name_row: adw::EntryRow,
    name_error: gtk4::Label,

    // Step 2
    want_hostname: adw::SwitchRow,
    hostname_form: gtk4::Box,
    zone_row: adw::ComboRow,
    zones: Vec<Zone>,
    subdomain: adw::EntryRow,
    scheme_row: adw::ComboRow,
    target_row: adw::EntryRow,
    port_row: adw::EntryRow,
    path_row: adw::EntryRow,
    hostname_preview: gtk4::Label,
    hostname_error: gtk4::Label,

    // Step 3
    review_list: gtk4::Box,

    // Step 4
    deploy_choice: adw::ComboRow,
    deploy_hosts: Vec<Option<i64>>,
    deploy_note: gtk4::Label,

    // Step 5
    result_box: gtk4::Box,
    outcome: RefCell<Option<CreateOutcome>>,
}

impl Wizard {
    fn hostname_wanted(&self) -> bool {
        self.want_hostname.is_active()
    }

    fn scheme(&self) -> OriginScheme {
        OriginScheme::SELECTABLE
            .get(self.scheme_row.selected() as usize)
            .copied()
            .unwrap_or(OriginScheme::Http)
    }

    fn zone(&self) -> Option<&Zone> {
        self.zones.get(self.zone_row.selected() as usize)
    }

    fn hostname(&self) -> Result<String, validation::ValidationError> {
        match self.zone() {
            Some(zone) => validation::subdomain(&self.subdomain.text(), &zone.name),
            None => validation::hostname(&self.subdomain.text()),
        }
    }

    fn service(&self) -> Result<OriginService, validation::ValidationError> {
        OriginService::from_fields(
            self.scheme(),
            &self.target_row.text(),
            &self.port_row.text(),
        )
    }

    fn request(&self) -> Option<CreateRequest> {
        let account_id = self.state.account_id()?;
        let name = validation::tunnel_name(&self.name_row.text()).ok()?;

        let hostname = if self.hostname_wanted() {
            let hostname = self.hostname().ok()?;
            let service = self.service().ok()?;
            let path = validation::ingress_path(&self.path_row.text()).ok()?;
            // A hostname whose zone is not on this account is allowed: the
            // ingress rule still goes in, and the review step says the DNS
            // record has to be made elsewhere.
            let zone_id = self
                .state
                .session()
                .and_then(|session| session.zone_for(&hostname).map(|zone| zone.id.clone()));

            Some(HostnameRequest {
                hostname,
                path,
                service,
                zone_id,
            })
        } else {
            None
        };

        Some(CreateRequest {
            account_id,
            name,
            hostname,
        })
    }

    fn validate_step(&self) {
        match self.step.get() {
            0 => {
                let result = validation::tunnel_name(&self.name_row.text());
                let started = !self.name_row.text().is_empty();
                match result {
                    Ok(_) => {
                        self.name_error.set_visible(false);
                        self.next.set_sensitive(true);
                    }
                    Err(error) => {
                        self.name_error.set_label(error.message());
                        self.name_error.set_visible(started);
                        self.next.set_sensitive(false);
                    }
                }
            }

            1 => {
                self.hostname_form.set_visible(self.hostname_wanted());
                if !self.hostname_wanted() {
                    self.hostname_error.set_visible(false);
                    self.next.set_sensitive(true);
                    return;
                }

                let scheme = self.scheme();
                match scheme.target_kind() {
                    TargetKind::HostPort => {
                        self.target_row.set_title("Origin host");
                        self.target_row.set_visible(true);
                        self.port_row.set_visible(true);
                    }
                    TargetKind::SocketPath => {
                        self.target_row.set_title("Socket path");
                        self.target_row.set_visible(true);
                        self.port_row.set_visible(false);
                    }
                    TargetKind::StatusCode => {
                        self.target_row.set_title("HTTP status code");
                        self.target_row.set_visible(true);
                        self.port_row.set_visible(false);
                    }
                    TargetKind::None => {
                        self.target_row.set_visible(false);
                        self.port_row.set_visible(false);
                    }
                }
                self.path_row.set_visible(scheme.supports_path());

                match self
                    .hostname()
                    .and_then(|hostname| self.service().map(|service| (hostname, service)))
                {
                    Ok((hostname, service)) => {
                        self.hostname_preview
                            .set_label(&format!("{hostname}  →  {}", service.to_service_string()));
                        self.hostname_error.set_visible(false);
                        self.next.set_sensitive(true);
                    }
                    Err(error) => {
                        self.hostname_preview.set_label("—");
                        self.hostname_error.set_label(error.message());
                        self.hostname_error.set_visible(true);
                        self.next.set_sensitive(false);
                    }
                }
            }

            2 => {
                // The review step re-checks the whole request: the steps
                // before it validate their own fields, but a change
                // underneath (e.g. the session going away) can still make
                // the request unbuildable. Creating with `None` would
                // silently do nothing.
                if self.request().is_none() {
                    self.next.set_sensitive(false);
                } else {
                    self.next.set_sensitive(true);
                }
            }
            _ => self.next.set_sensitive(true),
        }
    }

    fn show_step(&self, step: usize) {
        self.step.set(step);
        self.stack.set_visible_child_name(&format!("step-{step}"));

        self.heading.set_label(STEPS[step]);
        self.progress
            .set_label(&format!("Step {} of {}", step + 1, STEPS.len()));

        self.back.set_visible(step > 0 && step < 4);
        self.next.set_label(match step {
            2 => "Create Tunnel",
            3 => "Finish",
            4 => "Done",
            _ => "Next",
        });
        if step == 2 {
            self.next.add_css_class("suggested-action");
        } else {
            self.next.remove_css_class("suggested-action");
        }

        if step == 2 {
            self.build_review();
        }
        if step == 3 {
            self.update_deploy_step();
        }

        self.validate_step();
    }

    fn build_review(&self) {
        while let Some(child) = self.review_list.first_child() {
            self.review_list.remove(&child);
        }

        let Some(request) = self.request() else {
            self.review_list.append(&w::inline_message(
                "dialog-error-symbolic",
                "The tunnel details are not complete — go back a step and check \
                 the highlighted fields.",
                "error",
            ));
            return;
        };

        let group = w::group(
            "This will change your Cloudflare account",
            "Nothing has happened yet. Every change is listed below.",
        );

        for change in tunnels::review(&request) {
            let row = adw::ActionRow::builder().title(&change).build();
            row.set_title_lines(0);
            row.add_prefix(&gtk4::Image::from_icon_name("list-add-symbolic"));
            group.add(&row);
        }
        self.review_list.append(&group);

        let deploy = w::group("Deployment", "");
        let row = adw::ActionRow::builder()
            .title("No connector is deployed by this step")
            .subtitle(
                "A tunnel with no connector is created inactive, which is safe and \
                 expected. The next step offers to deploy one — or you can run \
                 cloudflared yourself later.",
            )
            .build();
        row.add_prefix(&gtk4::Image::from_icon_name("dialog-information-symbolic"));
        deploy.add(&row);
        self.review_list.append(&deploy);
    }

    fn update_deploy_step(&self) {
        let outcome = self.outcome.borrow();
        let Some(outcome) = outcome.as_ref() else {
            return;
        };

        let hosts = self.state.hosts();
        let trusted: Vec<_> = hosts.iter().filter(|host| host.is_trusted()).collect();

        self.deploy_note.set_label(&if trusted.is_empty() {
            format!(
                "No registered host is available, so “{}” has no connector yet. That is \
                 not a failure — the tunnel exists and is ready. Add a host from Remote \
                 Hosts, or run cloudflared yourself with the instructions on the next \
                 step.",
                outcome
                    .tunnel
                    .as_ref()
                    .map(|tunnel| tunnel.name.as_str())
                    .unwrap_or("the tunnel")
            )
        } else {
            "Deploying writes the tunnel's run token to a root-owned file on the chosen \
             host and installs a systemd unit that reads it. You will be shown every \
             command before anything runs."
                .to_string()
        });

        self.deploy_choice.set_sensitive(!trusted.is_empty());
    }

    fn build_result(&self) {
        while let Some(child) = self.result_box.first_child() {
            self.result_box.remove(&child);
        }

        let outcome = self.outcome.borrow();
        let Some(outcome) = outcome.as_ref() else {
            return;
        };

        let status = if outcome.succeeded() {
            w::inline_message("object-select-symbolic", "Everything completed.", "success")
        } else if outcome.is_partial() {
            w::inline_message(
                "dialog-warning-symbolic",
                "The tunnel was created, but not everything after it succeeded. What \
                 exists and what does not is listed below.",
                "warning",
            )
        } else {
            w::inline_message(
                "dialog-error-symbolic",
                "The tunnel was not created. Nothing was changed on your account.",
                "error",
            )
        };
        self.result_box.append(&status);

        let steps = w::group("What happened", "");
        for step in &outcome.steps {
            let row = adw::ActionRow::builder()
                .title(step.step.label())
                .subtitle(&step.detail)
                .build();
            row.set_subtitle_lines(0);
            let wrapper = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            wrapper.add_css_class(step.status.css_class());
            wrapper.set_valign(gtk4::Align::Center);
            wrapper.append(&gtk4::Image::from_icon_name(step.status.icon_name()));
            row.add_prefix(&wrapper);
            steps.add(&row);
        }
        self.result_box.append(&steps);

        if let Some(hostname) = &outcome.hostname {
            if let Some(recovery) = &hostname.recovery {
                let group = w::group("What to do next", "");
                let row = adw::ActionRow::builder()
                    .title(recovery.label())
                    .subtitle(recovery.explanation())
                    .build();
                row.set_subtitle_lines(0);
                row.add_prefix(&gtk4::Image::from_icon_name("dialog-warning-symbolic"));
                group.add(&row);
                self.result_box.append(&group);
            }
        }

        if let Some(instructions) = outcome.manual_instructions() {
            let group = w::group(
                "Run the connector",
                "A tunnel with no connector is inactive until something serves it.",
            );
            let expander = adw::ExpanderRow::builder()
                .title("Manual cloudflared setup")
                .subtitle("No SSH access from this application needed")
                .build();
            let row = adw::ActionRow::builder().build();
            row.set_child(Some(&{
                let wrapper = gtk4::Box::builder()
                    .orientation(gtk4::Orientation::Vertical)
                    .margin_top(12)
                    .margin_bottom(12)
                    .margin_start(12)
                    .margin_end(12)
                    .build();
                wrapper.append(&w::copyable_code_block(&instructions, ""));
                wrapper
            }));
            expander.add_row(&row);
            group.add(&expander);
            self.result_box.append(&group);
        }
    }
}

pub fn present(parent: &impl IsA<gtk4::Widget>, state: &AppState) {
    if !state.is_connected() {
        w::inform(
            parent,
            "Not connected",
            "Connect to Cloudflare before creating a tunnel.",
        );
        return;
    }

    let dialog = adw::Dialog::builder()
        .title("Create Tunnel")
        .content_width(620)
        // The first two steps are short; the later ones scroll. 700 filled
        // almost an entire 720px window with empty space on step 1.
        .content_height(560)
        .build();

    let stack = gtk4::Stack::new();
    stack.set_hhomogeneous(false);
    stack.set_vhomogeneous(false);
    stack.set_transition_type(gtk4::StackTransitionType::SlideLeftRight);

    // ── Step 1: the tunnel ───────────────────────────────────────────────
    let account_row = adw::ActionRow::builder().title("Account").build();
    account_row.add_css_class("property");
    account_row.set_subtitle(
        &state
            .connection()
            .account()
            .map(|account| format!("{} · {}", account.name, account.short_id()))
            .unwrap_or_else(|| "Not connected".into()),
    );

    let name_row = adw::EntryRow::builder().title("Tunnel name").build();
    let name_error = error_label();

    let step1 = step_column();
    let group = w::group(
        "Tunnel",
        "The tunnel is created with its configuration held by Cloudflare, so its \
         hostnames can be managed from here rather than by editing a file on a server.",
    );
    group.add(&account_row);
    group.add(&name_row);

    let managed = adw::ActionRow::builder()
        .title("Remotely managed configuration")
        .subtitle(
            "Always on. A locally configured tunnel reads a config.yml on its own \
             machine and would ignore anything written from here.",
        )
        .build();
    managed.add_prefix(&gtk4::Image::from_icon_name("object-select-symbolic"));
    group.add(&managed);

    step1.append(&group);
    step1.append(&name_error);
    stack.add_named(&scrolled(&step1), Some("step-0"));

    // ── Step 2: the optional hostname ────────────────────────────────────
    let step2 = step_column();
    let want_hostname = adw::SwitchRow::builder()
        .title("Publish a public hostname now")
        .subtitle("Optional. A tunnel can be created first and given hostnames later.")
        .active(false)
        .build();

    let toggle_group = w::group("Public hostname", "");
    toggle_group.add(&want_hostname);
    step2.append(&toggle_group);

    let zones = state.zones();
    let zone_names = gtk4::StringList::new(&[]);
    for zone in &zones {
        zone_names.append(&zone.name);
    }
    let zone_row = adw::ComboRow::builder()
        .title("Domain")
        .model(&zone_names)
        .build();
    if let Some(default) = state.settings().default_zone_id {
        if let Some(index) = zones.iter().position(|zone| zone.id == default) {
            zone_row.set_selected(index as u32);
        }
    }

    let subdomain = adw::EntryRow::builder().title("Subdomain").build();

    let scheme_names = gtk4::StringList::new(&[]);
    for scheme in OriginScheme::SELECTABLE {
        scheme_names.append(scheme.label());
    }
    let scheme_row = adw::ComboRow::builder()
        .title("Origin protocol")
        .model(&scheme_names)
        .build();

    let target_row = adw::EntryRow::builder().title("Origin host").build();
    target_row.set_text("localhost");
    let port_row = adw::EntryRow::builder().title("Origin port").build();
    port_row.set_text("80");
    let path_row = adw::EntryRow::builder()
        .title("Path prefix (optional)")
        .build();

    let hostname_form = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(18)
        .visible(false)
        .build();

    let hostname_group = w::group("Hostname", "");
    hostname_group.add(&zone_row);
    hostname_group.add(&subdomain);
    if zones.is_empty() {
        let warn = adw::ActionRow::builder()
            .title("No domains visible")
            .subtitle(
                "This token cannot see any zones, so no DNS record will be created. The \
                 ingress rule is still written and the record can be made wherever the \
                 domain is managed.",
            )
            .build();
        warn.add_prefix(&gtk4::Image::from_icon_name("dialog-warning-symbolic"));
        hostname_group.add(&warn);
    }
    hostname_form.append(&hostname_group);

    let origin_group = w::group(
        "Origin service",
        "As seen from the machine that will run the connector.",
    );
    origin_group.add(&scheme_row);
    origin_group.add(&target_row);
    origin_group.add(&port_row);
    origin_group.add(&path_row);
    hostname_form.append(&origin_group);

    let hostname_preview = gtk4::Label::builder()
        .halign(gtk4::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    hostname_preview.add_css_class("monospace");
    let preview_group = w::group("This will publish", "");
    let preview_row = adw::ActionRow::builder().build();
    preview_row.set_child(Some(&{
        let wrapper = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        wrapper.append(&hostname_preview);
        wrapper
    }));
    preview_group.add(&preview_row);
    hostname_form.append(&preview_group);

    let hostname_error = error_label();
    hostname_form.append(&hostname_error);

    step2.append(&hostname_form);
    stack.add_named(&scrolled(&step2), Some("step-1"));

    // ── Step 3: review ───────────────────────────────────────────────────
    let step3 = step_column();
    let review_list = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(18)
        .build();
    step3.append(&review_list);
    stack.add_named(&scrolled(&step3), Some("step-2"));

    // ── Step 4: optional deployment ──────────────────────────────────────
    let step4 = step_column();
    let deploy_group = w::group(
        "Deploy to a remote Linux host now?",
        "Entirely optional. SSH is not required to finish creating a tunnel.",
    );

    let hosts = state.hosts();
    let choices = gtk4::StringList::new(&["Skip for now"]);
    let mut deploy_hosts: Vec<Option<i64>> = vec![None];
    for host in &hosts {
        if host.is_trusted() {
            choices.append(&format!("{} ({})", host.name, host.display_target()));
            deploy_hosts.push(Some(host.id));
        }
    }
    let deploy_choice = adw::ComboRow::builder()
        .title("Deployment")
        .model(&choices)
        .build();
    deploy_group.add(&deploy_choice);

    let configure = adw::ButtonRow::builder()
        .title("Configure a New Remote Host")
        .start_icon_name("network-server-symbolic")
        .build();
    configure.set_action_name(Some("win.add-host"));
    deploy_group.add(&configure);

    let deploy_note = gtk4::Label::builder()
        .halign(gtk4::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .build();
    deploy_note.add_css_class("caption");
    deploy_note.add_css_class("dimmed");

    step4.append(&deploy_group);
    step4.append(&deploy_note);
    stack.add_named(&scrolled(&step4), Some("step-3"));

    // ── Step 5: result ───────────────────────────────────────────────────
    let step5 = step_column();
    let result_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(18)
        .build();
    step5.append(&result_box);
    stack.add_named(&scrolled(&step5), Some("step-4"));

    // ── Chrome ───────────────────────────────────────────────────────────
    let heading = gtk4::Label::builder().build();
    heading.add_css_class("title-4");
    let progress = gtk4::Label::builder().build();
    progress.add_css_class("caption");
    progress.add_css_class("dimmed");

    let title = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .valign(gtk4::Align::Center)
        .build();
    title.append(&heading);
    title.append(&progress);

    let back = gtk4::Button::builder().label("Back").build();
    let next = gtk4::Button::builder().label("Next").build();

    let header = adw::HeaderBar::builder().title_widget(&title).build();
    header.pack_start(&back);
    header.pack_end(&next);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    dialog.set_child(Some(&toolbar));

    let wizard = Rc::new(Wizard {
        state: state.clone(),
        dialog: dialog.clone(),
        stack,
        step: Cell::new(0),
        heading,
        progress,
        back: back.clone(),
        next: next.clone(),
        name_row: name_row.clone(),
        name_error,
        want_hostname: want_hostname.clone(),
        hostname_form,
        zone_row: zone_row.clone(),
        zones,
        subdomain: subdomain.clone(),
        scheme_row: scheme_row.clone(),
        target_row: target_row.clone(),
        port_row: port_row.clone(),
        path_row: path_row.clone(),
        hostname_preview,
        hostname_error,
        review_list,
        deploy_choice: deploy_choice.clone(),
        deploy_hosts,
        deploy_note,
        result_box,
        outcome: RefCell::new(None),
    });

    let revalidate = {
        let wizard = Rc::clone(&wizard);
        move || wizard.validate_step()
    };

    for entry in [&name_row, &subdomain, &target_row, &port_row, &path_row] {
        let revalidate = revalidate.clone();
        entry.connect_changed(move |_| revalidate());
    }
    {
        let revalidate = revalidate.clone();
        want_hostname.connect_active_notify(move |_| revalidate());
    }
    {
        let revalidate = revalidate.clone();
        scheme_row.connect_selected_notify(move |_| revalidate());
    }
    {
        let revalidate = revalidate.clone();
        zone_row.connect_selected_notify(move |_| revalidate());
    }

    {
        let wizard = Rc::clone(&wizard);
        back.connect_clicked(move |_| {
            let step = wizard.step.get();
            if step > 0 {
                wizard.show_step(step - 1);
            }
        });
    }

    {
        let wizard = Rc::clone(&wizard);
        next.connect_clicked(move |button| {
            let step = wizard.step.get();
            match step {
                0 => wizard.show_step(1),
                1 => wizard.show_step(2),
                2 => {
                    button.set_sensitive(false);
                    run_create(Rc::clone(&wizard));
                }
                3 => {
                    // Deployment, if a host was chosen. Skipping is the
                    // default and goes straight to the result.
                    let index = wizard.deploy_choice.selected() as usize;
                    match wizard.deploy_hosts.get(index).copied().flatten() {
                        None => wizard.show_step(4),
                        Some(host_id) => {
                            button.set_sensitive(false);
                            run_deploy(Rc::clone(&wizard), host_id);
                        }
                    };
                }
                _ => {
                    // `AdwDialog::close` returns whether it actually closed;
                    // the arm has to discard it to match the others.
                    wizard.dialog.close();
                }
            }
        });
    }

    wizard.show_step(0);
    dialog.present(Some(parent.as_ref()));
}

fn run_create(wizard: Rc<Wizard>) {
    let Some(request) = wizard.request() else {
        wizard.next.set_sensitive(true);
        while let Some(child) = wizard.review_list.first_child() {
            wizard.review_list.remove(&child);
        }
        wizard.review_list.append(&w::inline_message(
            "dialog-error-symbolic",
            "The tunnel details are no longer valid — go back, check the name and \
             hostname, then try again.",
            "error",
        ));
        return;
    };
    let Some(session) = wizard.state.session() else {
        return;
    };

    wizard.heading.set_label("Creating…");
    wizard.next.set_label("Working…");

    let client = session.client.clone();
    let state = wizard.state.clone();
    let cancel = Cancel::new();

    runtime::spawn(
        async move { tunnels::create(&client, &request, &cancel).await },
        move |outcome| {
            for step in &outcome.steps {
                let event = match step.status {
                    crate::services::hostnames::Status::Failed => ActivityEvent::error(
                        ActivityCategory::Tunnel,
                        format!("{}: {}", step.step.label(), step.detail),
                    ),
                    crate::services::hostnames::Status::Skipped => ActivityEvent::info(
                        ActivityCategory::Tunnel,
                        format!("{} skipped: {}", step.step.label(), step.detail),
                    ),
                    crate::services::hostnames::Status::Done => ActivityEvent::success(
                        ActivityCategory::Tunnel,
                        format!("{}: {}", step.step.label(), step.detail),
                    ),
                };
                state.record(event);
            }

            let created = outcome.tunnel.is_some();
            wizard.outcome.replace(Some(outcome));
            wizard.next.set_sensitive(true);
            wizard.build_result();

            crate::ui::refresh_tunnels(&state);

            // A tunnel that was never created has nothing to deploy to, so the
            // deployment step is skipped rather than offered and then refused.
            wizard.show_step(if created { 3 } else { 4 });
        },
    );
}

fn run_deploy(wizard: Rc<Wizard>, host_id: i64) {
    let state = wizard.state.clone();
    let Some(host) = state.host(host_id) else {
        wizard.show_step(4);
        return;
    };
    let outcome = wizard.outcome.borrow().clone();
    let (Some(outcome), Some(session)) = (outcome, state.session()) else {
        wizard.show_step(4);
        return;
    };
    let Some(tunnel) = outcome.tunnel.clone() else {
        wizard.show_step(4);
        return;
    };

    wizard.heading.set_label("Planning the deployment…");

    let controller = state.controller();
    let account_id = state.account_id().unwrap_or_default();
    let client = session.client.clone();
    let wizard_ref = Rc::clone(&wizard);
    let cancel = Cancel::new();
    let tunnel_id = tunnel.id.clone();
    let tunnel_name = tunnel.name.clone();
    let host_for_plan = host.clone();

    runtime::spawn(
        async move {
            let passphrase = runtime::blocking(move || {
                crate::storage::secrets::load_host_passphrase(host_id)
                    .ok()
                    .flatten()
            })
            .await
            .flatten();

            let plan = crate::services::hosts::plan_deployment(
                &controller,
                &host_for_plan,
                passphrase.clone(),
                &tunnel_id,
                &tunnel_name,
            )
            .await;

            let token = crate::services::tunnels::fetch_run_token(
                &client,
                &account_id,
                &tunnel_id,
                &cancel,
            )
            .await;

            (plan, token, passphrase)
        },
        move |(plan, token, passphrase)| {
            wizard_ref.next.set_sensitive(true);
            wizard_ref.heading.set_label("Deploy");

            let (plan, token) = match (plan, token) {
                (Ok(plan), Ok(token)) => (plan, token),
                (Err(error), _) => {
                    state.record(ActivityEvent::error(
                        ActivityCategory::RemoteService,
                        error.user_message(),
                    ));
                    w::inform(&wizard_ref.dialog, error.title(), &error.user_message());
                    wizard_ref.show_step(4);
                    return;
                }
                (_, Err(error)) => {
                    state.record(ActivityEvent::error(
                        ActivityCategory::Tunnel,
                        error.user_message(),
                    ));
                    w::inform(&wizard_ref.dialog, error.title(), &error.user_message());
                    wizard_ref.show_step(4);
                    return;
                }
            };

            crate::ui::deployment_dialog::present(
                &wizard_ref.dialog,
                &state,
                &host,
                plan,
                token,
                passphrase,
                {
                    let wizard = Rc::clone(&wizard_ref);
                    move || wizard.show_step(4)
                },
            );
        },
    );
}

fn step_column() -> gtk4::Box {
    gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(18)
        .margin_top(18)
        .margin_bottom(24)
        .margin_start(18)
        .margin_end(18)
        .build()
}

fn scrolled(child: &gtk4::Box) -> gtk4::ScrolledWindow {
    gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(child)
        .build()
}

fn error_label() -> gtk4::Label {
    let label = gtk4::Label::builder()
        .halign(gtk4::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    label.add_css_class("error");
    label.add_css_class("caption");
    label
}
