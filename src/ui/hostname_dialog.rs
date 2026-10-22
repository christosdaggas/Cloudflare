//! Publishing and editing a public hostname.
//!
//! The form is deliberately field-by-field rather than one "service URL" box.
//! A single text field would accept `localhost:8080` (no scheme),
//! `http://localhost` (no port, where the scheme has no default the user
//! meant) and `unix:///run/app.sock` (one slash too many, and silently wrong),
//! and it would validate all three only after a round trip. Separate fields
//! make the shape of a service explicit and let each part be checked as it is
//! typed.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::cloudflare::Cancel;
use crate::domain::activity::ActivityCategory;
use crate::domain::config::{OriginRequest, OriginRequestUpdate};
use crate::domain::hostname::{OriginScheme, OriginService, PublicHostname, TargetKind};
use crate::domain::tunnel::Tunnel;
use crate::domain::validation;
use crate::domain::Zone;
use crate::services::hostnames::{self, PublishRequest};
use crate::services::runtime;
use crate::ui::state::AppState;
use crate::ui::widgets as w;

/// The widgets the form needs to read back, kept together so the handlers can
/// share one `Rc` rather than capturing eight things each.
struct Form {
    zone_row: adw::ComboRow,
    zones: Vec<Zone>,
    subdomain: adw::EntryRow,
    preview: gtk4::Label,
    scheme_row: adw::ComboRow,
    target_row: adw::EntryRow,
    port_row: adw::EntryRow,
    path_row: adw::EntryRow,
    host_header_row: adw::EntryRow,
    sni_row: adw::EntryRow,
    no_tls_verify_row: adw::SwitchRow,
    http2_row: adw::SwitchRow,
    disable_chunked_row: adw::SwitchRow,
    /// The rule's existing `originRequest` when editing, so settings made
    /// elsewhere (timeouts, a proxy, …) survive a save that never touched
    /// them.
    base_origin_request: Option<OriginRequest>,
    error: gtk4::Label,
    publish: gtk4::Button,
    /// Set when editing, so the hostname fields can be locked: moving a
    /// hostname is a different, riskier operation than changing its origin.
    editing: Option<PublicHostname>,
}

impl Form {
    fn scheme(&self) -> OriginScheme {
        OriginScheme::SELECTABLE
            .get(self.scheme_row.selected() as usize)
            .copied()
            .unwrap_or(OriginScheme::Http)
    }

    fn zone(&self) -> Option<&Zone> {
        self.zones.get(self.zone_row.selected() as usize)
    }

    /// The hostname the form currently describes, or the reason it is not
    /// valid yet.
    fn hostname(&self) -> Result<String, validation::ValidationError> {
        if let Some(editing) = &self.editing {
            return Ok(editing.hostname.clone());
        }
        let Some(zone) = self.zone() else {
            return Err(validation::ValidationError::new(
                "This account has no domains this token can see, so a hostname cannot \
                 be published from here.",
            ));
        };
        validation::subdomain(&self.subdomain.text(), &zone.name)
    }

    fn service(&self) -> Result<OriginService, validation::ValidationError> {
        OriginService::from_fields(
            self.scheme(),
            &self.target_row.text(),
            &self.port_row.text(),
        )
    }

    fn path(&self) -> Result<Option<String>, validation::ValidationError> {
        validation::ingress_path(&self.path_row.text())
    }

    /// The `originRequest` the form describes — the dashboard's "Additional
    /// application settings".
    ///
    /// Starts from the rule's existing one when editing, so settings made
    /// elsewhere (timeouts, a proxy, …) survive a save that never touched
    /// them. Explicitly clears `originRequest` when everything is at its
    /// default, so an edit cannot accidentally preserve settings the user
    /// removed.
    fn origin_request(&self) -> Result<OriginRequestUpdate, validation::ValidationError> {
        let mut request = self.base_origin_request.clone().unwrap_or_default();

        let header = self.host_header_row.text().trim().to_string();
        if header.is_empty() {
            request.http_host_header = None;
        } else {
            request.http_host_header = Some(validation::http_host_header(&header)?);
        }

        let sni = self.sni_row.text().trim().to_string();
        if sni.is_empty() {
            request.origin_server_name = None;
        } else {
            request.origin_server_name = Some(validation::origin_host(&sni)?);
        }

        request.no_tls_verify =
            switch_flag(self.no_tls_verify_row.is_active(), request.no_tls_verify);
        request.http2_origin = switch_flag(self.http2_row.is_active(), request.http2_origin);
        request.disable_chunked_encoding = switch_flag(
            self.disable_chunked_row.is_active(),
            request.disable_chunked_encoding,
        );

        if request.is_empty() {
            Ok(OriginRequestUpdate::Clear)
        } else {
            Ok(OriginRequestUpdate::Set(Box::new(request)))
        }
    }

    /// Re-runs every check and updates the preview, the error line and whether
    /// the button can be pressed.
    fn revalidate(&self) {
        let scheme = self.scheme();

        // The second field means something different per scheme, so its label
        // and its sibling's visibility follow it.
        match scheme.target_kind() {
            TargetKind::HostPort => {
                self.target_row.set_title("Origin host");
                self.target_row.set_visible(true);
                self.port_row.set_visible(true);
                if self.port_row.text().is_empty() {
                    if let Some(port) = scheme.default_port() {
                        self.port_row.set_text(&port.to_string());
                    }
                }
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
                if self.target_row.text().is_empty() {
                    self.target_row.set_text("404");
                }
            }
            TargetKind::None => {
                self.target_row.set_visible(false);
                self.port_row.set_visible(false);
            }
        }

        self.path_row.set_visible(scheme.supports_path());

        let outcome = self
            .hostname()
            .and_then(|hostname| self.service().map(|service| (hostname, service)))
            .and_then(|(hostname, service)| self.path().map(|path| (hostname, service, path)))
            .and_then(|(hostname, service, path)| {
                self.origin_request()
                    .map(|origin| (hostname, service, path, origin))
            });

        match outcome {
            Ok((hostname, service, path, origin)) => {
                let display = match &path {
                    Some(path) => format!("{hostname}{path}"),
                    None => hostname,
                };
                let mut text = format!("{display}  →  {}", service.to_service_string());
                if let OriginRequestUpdate::Set(origin) = &origin {
                    let details: Vec<String> = origin
                        .summary()
                        .into_iter()
                        .map(|(label, value)| format!("{label}: {value}"))
                        .collect();
                    text.push_str(&format!("\n{}", details.join(" · ")));
                }
                self.preview.set_label(&text);
                self.preview.remove_css_class("error");
                self.error.set_visible(false);
                self.publish.set_sensitive(true);
            }
            Err(error) => {
                self.preview.set_label("—");
                self.error.set_label(error.message());
                self.error.set_visible(true);
                self.publish.set_sensitive(false);
            }
        }
    }
}

/// A switch writes `true` when on and removes a previous `true` when off.
/// An explicit `false` from elsewhere is preserved verbatim.
fn switch_flag(active: bool, original: Option<bool>) -> Option<bool> {
    if active {
        Some(true)
    } else {
        original.filter(|value| !*value)
    }
}

/// Opens the form for a new hostname.
pub fn present_new<W>(parent: &W, state: &AppState, tunnel: &Tunnel, on_done: impl Fn(&W) + 'static)
where
    W: IsA<gtk4::Widget> + Clone + 'static,
{
    present(parent, state, tunnel, None, on_done);
}

/// Opens the form for an existing hostname, with its identity locked.
pub fn present_edit<W>(
    parent: &W,
    state: &AppState,
    tunnel: &Tunnel,
    hostname: &PublicHostname,
    on_done: impl Fn(&W) + 'static,
) where
    W: IsA<gtk4::Widget> + Clone + 'static,
{
    present(parent, state, tunnel, Some(hostname.clone()), on_done);
}

fn present<W>(
    parent: &W,
    state: &AppState,
    tunnel: &Tunnel,
    editing: Option<PublicHostname>,
    on_done: impl Fn(&W) + 'static,
) where
    W: IsA<gtk4::Widget> + Clone + 'static,
{
    let zones = state.zones();

    let dialog = adw::Dialog::builder()
        .title(if editing.is_some() {
            "Edit origin service"
        } else {
            "Publish a hostname"
        })
        .content_width(560)
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

    // ── Hostname ─────────────────────────────────────────────────────────
    let hostname_group = w::group(
        "Hostname",
        if editing.is_some() {
            "Moving a hostname to a different tunnel is not an edit. Unpublish it here \
             and publish it on the other tunnel, so the DNS record is changed \
             deliberately rather than as a side effect."
        } else {
            "The public address people will use. A proxied CNAME will be created for it."
        },
    );

    let zone_names = gtk4::StringList::new(&[]);
    for zone in &zones {
        zone_names.append(&zone.name);
    }
    let zone_row = adw::ComboRow::builder()
        .title("Domain")
        .model(&zone_names)
        .build();

    let subdomain = adw::EntryRow::builder().title("Subdomain").build();

    if let Some(existing) = &editing {
        zone_row.set_sensitive(false);
        subdomain.set_sensitive(false);
        subdomain.set_text(&existing.hostname);
        subdomain.set_title("Hostname");
        if let Some(index) = zones
            .iter()
            .position(|zone| Some(&zone.id) == existing.zone_id.as_ref())
        {
            zone_row.set_selected(index as u32);
        }
    } else if let Some(default) = state.settings().default_zone_id {
        if let Some(index) = zones.iter().position(|zone| zone.id == default) {
            zone_row.set_selected(index as u32);
        }
    }

    hostname_group.add(&zone_row);
    hostname_group.add(&subdomain);

    if zones.is_empty() && editing.is_none() {
        let warn = adw::ActionRow::builder()
            .title("No domains available")
            .subtitle(
                "This token cannot see any zones on the account, so no DNS record can be \
                 created. You can still add the ingress rule and create the record \
                 wherever the domain is managed.",
            )
            .build();
        warn.add_prefix(&gtk4::Image::from_icon_name("dialog-warning-symbolic"));
        hostname_group.add(&warn);
    }

    column.append(&hostname_group);

    // ── Origin ───────────────────────────────────────────────────────────
    let origin_group = w::group(
        "Origin service",
        "Where cloudflared sends the traffic, as seen from the machine running the \
         connector. `localhost` here means that machine, not this one.",
    );

    let scheme_names = gtk4::StringList::new(&[]);
    for scheme in OriginScheme::SELECTABLE {
        scheme_names.append(scheme.label());
    }
    let scheme_row = adw::ComboRow::builder()
        .title("Protocol")
        .model(&scheme_names)
        .build();

    let target_row = adw::EntryRow::builder().title("Origin host").build();
    let port_row = adw::EntryRow::builder().title("Port").build();
    let path_row = adw::EntryRow::builder()
        .title("Path prefix (optional)")
        .build();

    if let Some(existing) = &editing {
        let service = &existing.service;
        if let Some(index) = OriginScheme::SELECTABLE
            .iter()
            .position(|scheme| *scheme == service.scheme)
        {
            scheme_row.set_selected(index as u32);
        }
        target_row.set_text(&service.host);
        if let Some(port) = service.port {
            port_row.set_text(&port.to_string());
        }
        if let Some(path) = &existing.path {
            path_row.set_text(path);
        }
    } else {
        target_row.set_text("localhost");
        port_row.set_text("80");
    }

    origin_group.add(&scheme_row);
    origin_group.add(&target_row);
    origin_group.add(&port_row);
    origin_group.add(&path_row);
    column.append(&origin_group);

    // ── Origin behaviour ───────────────────────────────────────────────
    // The dashboard's "Additional application settings": how cloudflared
    // talks to the origin. Everything is optional — unset means
    // cloudflared's defaults.
    let behaviour_group = w::group(
        "Origin behaviour (optional)",
        "How cloudflared talks to the origin: the Host header it sends, the \
         name it expects on the origin's certificate, and the TLS behaviour. \
         Settings made elsewhere are kept as they are.",
    );

    let host_header_row = adw::EntryRow::builder().title("HTTP Host header").build();
    host_header_row.set_tooltip_text(Some(
        "The Host header sent to the origin. Empty means the public hostname.",
    ));
    let sni_row = adw::EntryRow::builder()
        .title("Origin server name (SNI)")
        .build();
    sni_row.set_tooltip_text(Some(
        "The name expected on the origin's certificate. Empty means the service URL.",
    ));
    let no_tls_verify_row = adw::SwitchRow::builder()
        .title("Skip TLS verification")
        .subtitle("Accept any certificate from the origin. Only for testing.")
        .build();
    let http2_row = adw::SwitchRow::builder()
        .title("Use HTTP/2 to the origin")
        .subtitle("Requires HTTPS on the origin.")
        .build();
    let disable_chunked_row = adw::SwitchRow::builder()
        .title("Disable chunked encoding")
        .subtitle("For origins that cannot handle it, such as WSGI servers.")
        .build();

    if let Some(origin) = editing
        .as_ref()
        .and_then(|existing| existing.origin_request.as_ref())
    {
        if let Some(header) = &origin.http_host_header {
            host_header_row.set_text(header);
        }
        if let Some(sni) = &origin.origin_server_name {
            sni_row.set_text(sni);
        }
        no_tls_verify_row.set_active(origin.no_tls_verify.unwrap_or(false));
        http2_row.set_active(origin.http2_origin.unwrap_or(false));
        disable_chunked_row.set_active(origin.disable_chunked_encoding.unwrap_or(false));
    }

    behaviour_group.add(&host_header_row);
    behaviour_group.add(&sni_row);
    behaviour_group.add(&no_tls_verify_row);
    behaviour_group.add(&http2_row);
    behaviour_group.add(&disable_chunked_row);
    column.append(&behaviour_group);

    // ── Preview and result ───────────────────────────────────────────────
    let preview_group = w::group("This will publish", "");
    let preview = gtk4::Label::builder()
        .halign(gtk4::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    preview.add_css_class("monospace");
    preview.add_css_class("body");

    let preview_row = adw::ActionRow::builder().build();
    preview_row.set_child(Some(&{
        let wrapper = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        wrapper.append(&preview);
        wrapper
    }));
    preview_group.add(&preview_row);
    column.append(&preview_group);

    let error = gtk4::Label::builder()
        .halign(gtk4::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    error.add_css_class("error");
    error.add_css_class("caption");
    column.append(&error);

    let publish = gtk4::Button::builder()
        .label(if editing.is_some() { "Save" } else { "Publish" })
        .halign(gtk4::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    column.append(&publish);

    let form = Rc::new(RefCell::new(Form {
        zone_row: zone_row.clone(),
        zones,
        subdomain: subdomain.clone(),
        preview,
        scheme_row: scheme_row.clone(),
        target_row: target_row.clone(),
        port_row: port_row.clone(),
        path_row: path_row.clone(),
        host_header_row: host_header_row.clone(),
        sni_row: sni_row.clone(),
        no_tls_verify_row: no_tls_verify_row.clone(),
        http2_row: http2_row.clone(),
        disable_chunked_row: disable_chunked_row.clone(),
        base_origin_request: editing
            .as_ref()
            .and_then(|existing| existing.origin_request.clone()),
        error,
        publish: publish.clone(),
        editing,
    }));

    // Every field revalidates the whole form: the fields are interdependent
    // (a scheme decides whether a port is needed, a zone decides what a
    // subdomain resolves to), so validating one at a time would be wrong.
    let revalidate = {
        let form = Rc::clone(&form);
        move || form.borrow().revalidate()
    };

    {
        let revalidate = revalidate.clone();
        scheme_row.connect_selected_notify(move |_| revalidate());
    }
    {
        let revalidate = revalidate.clone();
        zone_row.connect_selected_notify(move |_| revalidate());
    }
    for entry in [
        &subdomain,
        &target_row,
        &port_row,
        &path_row,
        &host_header_row,
        &sni_row,
    ] {
        let revalidate = revalidate.clone();
        entry.connect_changed(move |_| revalidate());
    }
    for switch in [&no_tls_verify_row, &http2_row, &disable_chunked_row] {
        let revalidate = revalidate.clone();
        switch.connect_active_notify(move |_| revalidate());
    }
    revalidate();

    {
        let form = Rc::clone(&form);
        let state = state.clone();
        let tunnel = tunnel.clone();
        let dialog = dialog.clone();
        let parent = parent.clone();
        let on_done = Rc::new(on_done);

        publish.connect_clicked(move |button| {
            let form = form.borrow();
            let (Ok(hostname), Ok(service), Ok(path), Ok(origin_request)) = (
                form.hostname(),
                form.service(),
                form.path(),
                form.origin_request(),
            ) else {
                return;
            };

            let zone_id = form.zone().map(|zone| zone.id.clone());
            let Some(account_id) = state.account_id() else {
                return;
            };
            let Some(session) = state.session() else {
                return;
            };

            button.set_sensitive(false);
            dialog.close();

            let request = PublishRequest {
                account_id,
                tunnel_id: tunnel.id.clone(),
                tunnel_name: tunnel.name.clone(),
                hostname,
                path,
                service,
                zone_id,
                origin_request,
            };

            let client = session.client.clone();
            let state = state.clone();
            let parent = parent.clone();
            let on_done = Rc::clone(&on_done);
            let cancel = Cancel::new();

            runtime::spawn(
                async move { hostnames::publish(&client, &request, &cancel).await },
                move |outcome| {
                    crate::ui::tunnel_detail_page::record_outcome(
                        &state,
                        &outcome,
                        ActivityCategory::TunnelConfig,
                    );
                    show_outcome(&parent, &outcome);
                    on_done(&parent);
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

/// Reports what happened, in proportion to how badly it went.
///
/// A clean success is a toast. A partial failure is a dialog listing every
/// step with the recovery the service recommended, because the user has to be
/// told which half of the change exists — a toast that vanishes in four
/// seconds is not adequate for "your tunnel was changed but DNS was not".
pub fn show_outcome(parent: &impl IsA<gtk4::Widget>, outcome: &hostnames::Outcome) {
    if outcome.succeeded() && outcome.recovery.is_none() {
        w::toast(parent, &outcome.summary());
        return;
    }

    let dialog = adw::AlertDialog::new(
        Some(if outcome.is_partial() {
            "Partly completed"
        } else {
            "Could not complete"
        }),
        Some(&outcome.summary()),
    );

    let column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .build();

    let steps = w::group("What happened", "");
    for step in &outcome.steps {
        let row = adw::ActionRow::builder()
            .title(step.step.label())
            .subtitle(&step.detail)
            .build();
        let wrapper = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        wrapper.add_css_class(step.status.css_class());
        wrapper.set_valign(gtk4::Align::Center);
        wrapper.append(&gtk4::Image::from_icon_name(step.status.icon_name()));
        row.add_prefix(&wrapper);
        steps.add(&row);
    }
    column.append(&steps);

    if let Some(recovery) = &outcome.recovery {
        column.append(&w::inline_message(
            "dialog-information-symbolic",
            &recovery.explanation(),
            "warning",
        ));
    }

    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(420)
        .child(&column)
        .build();

    dialog.set_extra_child(Some(&scroller));
    dialog.set_prefer_wide_layout(true);
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.present(Some(parent.as_ref()));
}

#[cfg(test)]
mod tests {
    use super::switch_flag;

    #[test]
    fn switch_flags_preserve_an_explicit_false() {
        assert_eq!(switch_flag(false, Some(false)), Some(false));
        assert_eq!(switch_flag(false, Some(true)), None);
        assert_eq!(switch_flag(false, None), None);
        assert_eq!(switch_flag(true, Some(false)), Some(true));
    }
}
