//! The deployment preview, and applying it.
//!
//! Nothing here runs until the user has read what will run. That is the whole
//! reason this dialog exists rather than a progress bar: a deployment writes
//! two root-owned files and installs a systemd unit on a machine the user
//! cares about, and "preview privileged changes and require confirmation" only
//! means something if the preview is the complete list.
//!
//! The tunnel's run token is the one secret involved. It is held in a
//! [`SecretString`], it travels over the SSH channel's stdin, and the preview
//! shows the command that receives it with the content replaced by a note
//! saying where it actually goes.

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use secrecy::SecretString;

use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::host::RemoteHost;
use crate::remote::controller::DeploymentPlan;
use crate::services::{hosts, runtime};
use crate::ui::state::AppState;
use crate::ui::widgets as w;

pub fn present(
    parent: &impl IsA<gtk4::Widget>,
    state: &AppState,
    host: &RemoteHost,
    plan: DeploymentPlan,
    token: SecretString,
    passphrase: Option<SecretString>,
    on_done: impl Fn() + 'static,
) {
    let dialog = adw::Dialog::builder()
        .title("Review the deployment")
        .content_width(720)
        .content_height(700)
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
        "dialog-warning-symbolic",
        &format!(
            "These commands will run on {} as root, through sudo. Read them before \
             continuing — nothing has happened yet.",
            host.display_target()
        ),
        "warning",
    ));

    let summary = w::group("What will be installed", "");
    summary.add(&w::property_row("Host", &host.display_target()));
    summary.add(&w::property_row("Tunnel", &plan.tunnel_name));
    summary.add(&w::property_row("cloudflared", &plan.cloudflared_path));
    summary.add(&w::property_row("Service unit", &plan.unit));
    summary.add(&w::property_row("Token file", &plan.token_path));
    column.append(&summary);

    let steps = w::group(
        "Commands",
        "In this order. Each one stops the deployment if it fails, and the result page \
         says which ones completed.",
    );
    for (index, step) in plan.steps.iter().enumerate() {
        let row = adw::ExpanderRow::builder()
            .title(format!("{}. {}", index + 1, step.description))
            .build();

        if step.privileged {
            row.add_prefix(&gtk4::Image::from_icon_name("channel-secure-symbolic"));
        }
        if step.overwrites {
            row.add_suffix(&w::status_badge(
                "dialog-warning-symbolic",
                "Replaces a file",
                "warning",
            ));
        }

        let command = adw::ActionRow::builder().build();
        command.set_child(Some(&{
            let wrapper = gtk4::Box::builder()
                .orientation(gtk4::Orientation::Vertical)
                .margin_top(12)
                .margin_bottom(12)
                .margin_start(12)
                .margin_end(12)
                .build();
            let label = gtk4::Label::builder()
                .label(&step.preview)
                .halign(gtk4::Align::Start)
                .xalign(0.0)
                .wrap(true)
                .selectable(true)
                .build();
            label.add_css_class("monospace");
            label.add_css_class("caption");
            wrapper.append(&label);
            wrapper
        }));
        row.add_row(&command);
        steps.add(&row);
    }
    column.append(&steps);

    let unit_group = w::group(
        "The systemd unit",
        "Written verbatim. It reads the token from the file above rather than taking it \
         as an argument, so the token never appears in the process table.",
    );
    let unit_row = adw::ActionRow::builder().build();
    unit_row.set_child(Some(&{
        let wrapper = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        wrapper.append(&w::copyable_code_block(&plan.unit_file, ""));
        wrapper
    }));
    unit_group.add(&unit_row);
    column.append(&unit_group);

    let confirm_overwrite = adw::SwitchRow::builder()
        .title("Replace the existing unit file")
        .subtitle(format!(
            "There is already a file at {}. Deployment will not proceed without this.",
            plan.unit_path
        ))
        .active(false)
        .visible(plan.replaces_existing_unit)
        .build();
    let overwrite_group = w::group("", "");
    overwrite_group.add(&confirm_overwrite);
    overwrite_group.set_visible(plan.replaces_existing_unit);
    column.append(&overwrite_group);

    let apply = gtk4::Button::builder()
        .label("Deploy")
        .halign(gtk4::Align::Center)
        .css_classes(["pill", "destructive-action"])
        .build();
    apply.set_sensitive(!plan.replaces_existing_unit);
    column.append(&apply);

    {
        let apply = apply.clone();
        confirm_overwrite.connect_active_notify(move |row| apply.set_sensitive(row.is_active()));
    }

    {
        let state = state.clone();
        let host = host.clone();
        let dialog_ref = dialog.clone();
        let parent = parent.as_ref().clone();
        let confirm_overwrite = confirm_overwrite.clone();
        let on_done = std::rc::Rc::new(on_done);

        apply.connect_clicked(move |button| {
            button.set_sensitive(false);
            button.set_label("Deploying…");

            let controller = state.controller();
            let host = host.clone();
            let plan = plan.clone();
            let token = token.clone();
            let passphrase = passphrase.clone();
            let confirmed = confirm_overwrite.is_active();
            let state = state.clone();
            let dialog = dialog_ref.clone();
            let parent = parent.clone();
            let on_done = std::rc::Rc::clone(&on_done);

            runtime::spawn(
                async move {
                    hosts::apply_deployment(
                        &controller,
                        &host,
                        passphrase,
                        &plan,
                        &token,
                        confirmed,
                    )
                    .await
                },
                move |result| {
                    dialog.close();

                    match result {
                        Ok(outcome) => {
                            for step in &outcome.completed {
                                state.record(ActivityEvent::success(
                                    ActivityCategory::RemoteService,
                                    format!("Deployment: {step}"),
                                ));
                            }
                            match &outcome.failed {
                                None => {
                                    state.record(ActivityEvent::success(
                                        ActivityCategory::RemoteService,
                                        "Deployment completed",
                                    ));
                                    w::toast(&parent, "Connector deployed and started.");
                                }
                                Some((step, detail)) => {
                                    state.record(ActivityEvent::error(
                                        ActivityCategory::RemoteService,
                                        format!("Deployment failed at “{step}”: {detail}"),
                                    ));
                                    w::inform(
                                        &parent,
                                        "Deployment did not finish",
                                        &format!(
                                            "{} completed, then “{step}” failed: {detail}\n\n\
                                             Nothing further was attempted. The host is \
                                             partly configured; running the deployment \
                                             again is safe — each step overwrites its \
                                             own file rather than appending.",
                                            w::plural(outcome.completed.len(), "step", "steps")
                                        ),
                                    );
                                }
                            }
                            crate::ui::hosts_page::refresh_hosts(&state);
                        }
                        Err(error) => {
                            state.record(ActivityEvent::error(
                                ActivityCategory::RemoteService,
                                error.user_message(),
                            ));
                            w::inform(&parent, error.title(), &error.user_message());
                        }
                    }

                    on_done();
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
