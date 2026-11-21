use anyhow::{Context, Result};
use gtk::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

const BUTTONS: &[(&str, &str)] = &[
    ("left", "Sinistro"),
    ("right", "Destro"),
    ("middle", "Centrale"),
    ("back", "Laterale posteriore"),
    ("forward", "Laterale anteriore"),
];
const TOGGLE_BUTTONS: &[(&str, &str)] = &[
    ("left", "Sinistro"),
    ("right", "Destro"),
    ("middle", "Centrale"),
    ("back", "Laterale posteriore"),
    ("forward", "Laterale anteriore"),
    ("none", "Nessuno (disattivato)"),
];
const OSD_MODES: &[(&str, &str)] = &[
    ("overlay", "Overlay sul desktop"),
    ("notification", "Notifica KDE"),
    ("none", "Nessun indicatore"),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ButtonRule {
    button: String,
    clicks: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GuiConfig {
    #[serde(default = "zero_schema_version")]
    schema_version: u32,
    #[serde(default = "default_device")]
    device: String,
    #[serde(default = "empty_rules")]
    double_rules: Vec<ButtonRule>,
    #[serde(default = "empty_rules")]
    triple_rules: Vec<ButtonRule>,
    #[serde(default = "default_double_toggle")]
    double_toggle_button: String,
    #[serde(default = "default_triple_toggle")]
    triple_toggle_button: String,
    #[serde(default = "default_double_enabled")]
    double_enabled: bool,
    #[serde(default = "default_triple_enabled")]
    triple_enabled: bool,
    #[serde(default = "default_osd")]
    osd: String,
    #[serde(default = "default_gap")]
    gap_ms: u64,
    #[serde(default = "default_press")]
    press_ms: u64,
    #[serde(default = "default_hold")]
    max_hold_ms: u64,
    // Campi legacy letti dalle configurazioni 0.5.x ma non più salvati.
    #[serde(rename = "click_button", default = "default_legacy_button", skip_serializing)]
    legacy_click_button: String,
    #[serde(rename = "double_clicks", default = "default_double_clicks", skip_serializing)]
    legacy_double_clicks: u8,
    #[serde(rename = "triple_clicks", default = "default_triple_clicks", skip_serializing)]
    legacy_triple_clicks: u8,
}

fn empty_rules() -> Vec<ButtonRule> { Vec::new() }
fn zero_schema_version() -> u32 { 0 }
fn default_device() -> String { "auto".to_string() }
fn default_double_toggle() -> String { "back".to_string() }
fn default_triple_toggle() -> String { "forward".to_string() }
fn default_double_enabled() -> bool { true }
fn default_triple_enabled() -> bool { false }
fn default_osd() -> String { "overlay".to_string() }
fn default_gap() -> u64 { 45 }
fn default_press() -> u64 { 12 }
fn default_hold() -> u64 { 250 }
fn default_legacy_button() -> String { "left".to_string() }
fn default_double_clicks() -> u8 { 2 }
fn default_triple_clicks() -> u8 { 3 }

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            device: "auto".to_string(),
            double_rules: vec![ButtonRule { button: "left".to_string(), clicks: 2 }],
            triple_rules: vec![ButtonRule { button: "left".to_string(), clicks: 3 }],
            double_toggle_button: "back".to_string(),
            triple_toggle_button: "forward".to_string(),
            double_enabled: true,
            triple_enabled: false,
            osd: "overlay".to_string(),
            gap_ms: 45,
            press_ms: 12,
            max_hold_ms: 250,
            legacy_click_button: "left".to_string(),
            legacy_double_clicks: 2,
            legacy_triple_clicks: 3,
        }
    }
}

#[derive(Clone)]
struct RuleWidgets {
    button: String,
    enabled: gtk::CheckButton,
    clicks: gtk::SpinButton,
}

pub fn run_gui() -> Result<()> {
    let app = gtk::Application::builder()
        .application_id("io.clickforge.config")
        .build();
    app.connect_activate(build_window);
    let _ = app.run_with_args(&["clickforge-gui"]);
    Ok(())
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("clickforge").join("config.toml")
}

fn log_path(config: &Path) -> PathBuf {
    config.parent().unwrap_or_else(|| Path::new(".")).join("clickforge.log")
}

fn load_config(path: &Path) -> (GuiConfig, Option<String>) {
    match fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<GuiConfig>(&contents) {
            Ok(mut config) => {
                if config.schema_version == 0 {
                    let legacy_button = if BUTTONS.iter().any(|(id, _)| *id == config.legacy_click_button.as_str()) {
                        config.legacy_click_button.clone()
                    } else {
                        "left".to_string()
                    };
                    if config.double_rules.is_empty() {
                        config.double_rules.push(ButtonRule {
                            button: legacy_button.clone(),
                            clicks: config.legacy_double_clicks.clamp(1, 20),
                        });
                    }
                    if config.triple_rules.is_empty() {
                        config.triple_rules.push(ButtonRule {
                            button: legacy_button,
                            clicks: config.legacy_triple_clicks.clamp(1, 20),
                        });
                    }
                    config.schema_version = 1;
                    (config, Some("Configurazione 0.5.x convertita al nuovo formato multi-pulsante. Salvala per completare l'aggiornamento.".to_string()))
                } else {
                    (config, None)
                }
            }
            Err(error) => (GuiConfig::default(), Some(format!("Configurazione non valida: {error}. Uso i valori predefiniti."))),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (GuiConfig::default(), None),
        Err(error) => (GuiConfig::default(), Some(format!("Non riesco a leggere la configurazione: {error}"))),
    }
}

fn save_config(path: &Path, config: &GuiConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("impossibile creare {}", parent.display()))?;
    }
    let encoded = toml::to_string_pretty(config).context("impossibile serializzare la configurazione TOML")?;
    fs::write(path, encoded).with_context(|| format!("impossibile salvare {}", path.display()))
}

fn make_combo(items: &[(&str, &str)], selected: &str) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    for (id, label) in items {
        combo.append(Some(id), label);
    }
    combo.set_active_id(Some(selected));
    if combo.active_id().is_none() {
        combo.set_active(Some(0));
    }
    combo
}

fn combo_value(combo: &gtk::ComboBoxText, fallback: &str) -> String {
    combo.active_id().map(|value| value.to_string()).unwrap_or_else(|| fallback.to_string())
}

fn labeled_row(grid: &gtk::Grid, row: i32, label: &str, widget: &impl IsA<gtk::Widget>) {
    let text = gtk::Label::new(Some(label));
    text.set_halign(gtk::Align::Start);
    text.set_valign(gtk::Align::Center);
    grid.attach(&text, 0, row, 1, 1);
    grid.attach(widget, 1, row, 1, 1);
}

fn build_profile_frame(title: &str, rules: &[ButtonRule]) -> (gtk::Frame, Vec<RuleWidgets>) {
    let frame = gtk::Frame::new(Some(title));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(10);
    content.set_margin_bottom(10);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let intro = gtk::Label::new(Some("Seleziona tutti i pulsanti da moltiplicare e imposta il conteggio per ciascuno."));
    intro.set_wrap(true);
    intro.set_halign(gtk::Align::Start);
    intro.add_css_class("dim-label");
    content.append(&intro);

    let grid = gtk::Grid::builder().column_spacing(12).row_spacing(8).build();
    let count_heading = gtk::Label::new(Some("Click totali"));
    count_heading.add_css_class("heading");
    grid.attach(&count_heading, 1, 0, 1, 1);

    let mut widgets = Vec::new();
    for (index, (id, label)) in BUTTONS.iter().enumerate() {
        let saved = rules.iter().find(|rule| rule.button == *id);
        let enabled = gtk::CheckButton::with_label(label);
        enabled.set_active(saved.is_some());

        let clicks = gtk::SpinButton::with_range(1.0, 20.0, 1.0);
        clicks.set_value(saved.map(|rule| rule.clicks.clamp(1, 20)).unwrap_or(2) as f64);
        clicks.set_sensitive(enabled.is_active());
        let clicks_for_toggle = clicks.clone();
        enabled.connect_toggled(move |check| clicks_for_toggle.set_sensitive(check.is_active()));

        let row = index as i32 + 1;
        grid.attach(&enabled, 0, row, 1, 1);
        grid.attach(&clicks, 1, row, 1, 1);
        widgets.push(RuleWidgets {
            button: (*id).to_string(),
            enabled,
            clicks,
        });
    }
    content.append(&grid);
    frame.set_child(Some(&content));
    (frame, widgets)
}

fn collect_rules(widgets: &[RuleWidgets]) -> Vec<ButtonRule> {
    widgets
        .iter()
        .filter(|item| item.enabled.is_active())
        .map(|item| ButtonRule {
            button: item.button.clone(),
            clicks: item.clicks.value_as_int().clamp(1, 20) as u8,
        })
        .collect()
}

fn collect_config(
    device: &gtk::Entry,
    double_rules: &[RuleWidgets],
    triple_rules: &[RuleWidgets],
    double_toggle: &gtk::ComboBoxText,
    triple_toggle: &gtk::ComboBoxText,
    double_enabled: &gtk::CheckButton,
    triple_enabled: &gtk::CheckButton,
    osd: &gtk::ComboBoxText,
    gap: &gtk::SpinButton,
    press: &gtk::SpinButton,
    max_hold: &gtk::SpinButton,
) -> GuiConfig {
    let device_text = device.text().to_string();
    let device_text = if device_text.trim().is_empty() { "auto".to_string() } else { device_text.trim().to_string() };
    GuiConfig {
        schema_version: 1,
        device: device_text,
        double_rules: collect_rules(double_rules),
        triple_rules: collect_rules(triple_rules),
        double_toggle_button: combo_value(double_toggle, "back"),
        triple_toggle_button: combo_value(triple_toggle, "forward"),
        double_enabled: double_enabled.is_active(),
        triple_enabled: triple_enabled.is_active(),
        osd: combo_value(osd, "overlay"),
        gap_ms: gap.value_as_int().clamp(0, 1000) as u64,
        press_ms: press.value_as_int().clamp(1, 100) as u64,
        max_hold_ms: max_hold.value_as_int().clamp(1, 2000) as u64,
        legacy_click_button: "left".to_string(),
        legacy_double_clicks: 2,
        legacy_triple_clicks: 3,
    }
}

fn validate_config(config: &GuiConfig) -> Result<(), String> {
    if config.double_rules.is_empty() && config.triple_rules.is_empty() {
        return Err("Seleziona almeno un pulsante da moltiplicare in uno dei due profili.".to_string());
    }
    if config.double_toggle_button != "none" && config.double_toggle_button == config.triple_toggle_button {
        return Err("I pulsanti di attivazione dei profili 2x e 3x devono essere diversi.".to_string());
    }
    for (profile, rules) in [("2x", &config.double_rules), ("3x", &config.triple_rules)] {
        for (index, rule) in rules.iter().enumerate() {
            if !BUTTONS.iter().any(|(id, _)| *id == rule.button.as_str()) {
                return Err(format!("Pulsante non valido nel profilo {profile}: {}.", rule.button));
            }
            if !(1..=20).contains(&rule.clicks) {
                return Err(format!("Il conteggio del profilo {profile} deve essere compreso tra 1 e 20."));
            }
            if rules[..index].iter().any(|other| other.button == rule.button) {
                return Err(format!("Il pulsante {} è ripetuto nel profilo {profile}.", rule.button));
            }
        }
    }
    let toggle_buttons = [config.double_toggle_button.as_str(), config.triple_toggle_button.as_str()];
    for rule in config.double_rules.iter().chain(config.triple_rules.iter()) {
        if toggle_buttons.iter().any(|toggle| *toggle != "none" && *toggle == rule.button.as_str()) {
            return Err(format!("Il pulsante {} non può essere sia moltiplicato sia usato come interruttore.", rule.button));
        }
    }
    if config.double_enabled && config.double_toggle_button == "none" {
        return Err("Il profilo 2x è attivo all'avvio ma non ha un pulsante di attivazione.".to_string());
    }
    if config.triple_enabled && config.triple_toggle_button == "none" {
        return Err("Il profilo 3x è attivo all'avvio ma non ha un pulsante di attivazione.".to_string());
    }
    Ok(())
}


fn build_window(app: &gtk::Application) {
    let settings_path = config_path();
    let (config, load_warning) = load_config(&settings_path);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("ClickForge — Configurazione")
        .default_width(820)
        .default_height(850)
        .resizable(true)
        .build();

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.set_margin_top(22);
    root.set_margin_bottom(22);
    root.set_margin_start(24);
    root.set_margin_end(24);

    let heading = gtk::Label::new(Some("ClickForge"));
    heading.add_css_class("title-1");
    heading.set_halign(gtk::Align::Start);
    root.append(&heading);

    let subtitle = gtk::Label::new(Some("Assegna più pulsanti a ogni profilo e scegli un conteggio indipendente per ciascuno."));
    subtitle.set_halign(gtk::Align::Start);
    subtitle.set_wrap(true);
    subtitle.add_css_class("dim-label");
    root.append(&subtitle);

    let basic_grid = gtk::Grid::builder().column_spacing(22).row_spacing(10).build();
    let device = gtk::Entry::new();
    device.set_text(&config.device);
    device.set_placeholder_text(Some("auto oppure /dev/input/eventX"));
    labeled_row(&basic_grid, 0, "Mouse da usare", &device);

    let double_toggle = make_combo(TOGGLE_BUTTONS, &config.double_toggle_button);
    labeled_row(&basic_grid, 1, "Interruttore del profilo 2x", &double_toggle);
    let triple_toggle = make_combo(TOGGLE_BUTTONS, &config.triple_toggle_button);
    labeled_row(&basic_grid, 2, "Interruttore del profilo 3x", &triple_toggle);

    let double_enabled = gtk::CheckButton::with_label("Attivo all'avvio");
    double_enabled.set_active(config.double_enabled);
    labeled_row(&basic_grid, 3, "Stato iniziale 2x", &double_enabled);
    let triple_enabled = gtk::CheckButton::with_label("Attivo all'avvio");
    triple_enabled.set_active(config.triple_enabled);
    labeled_row(&basic_grid, 4, "Stato iniziale 3x", &triple_enabled);

    let osd = make_combo(OSD_MODES, &config.osd);
    labeled_row(&basic_grid, 5, "Indicatore di stato", &osd);
    root.append(&basic_grid);

    let profiles = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    profiles.set_homogeneous(true);
    let (double_frame, double_rule_widgets) = build_profile_frame("Profilo 2x", &config.double_rules);
    let (triple_frame, triple_rule_widgets) = build_profile_frame("Profilo 3x", &config.triple_rules);
    profiles.append(&double_frame);
    profiles.append(&triple_frame);
    root.append(&profiles);

    let timing_title = gtk::Label::new(Some("Temporizzazione"));
    timing_title.add_css_class("title-3");
    timing_title.set_halign(gtk::Align::Start);
    root.append(&timing_title);
    let timing_grid = gtk::Grid::builder().column_spacing(22).row_spacing(10).build();
    let gap = gtk::SpinButton::with_range(0.0, 1000.0, 5.0);
    gap.set_value(config.gap_ms.min(1000) as f64);
    labeled_row(&timing_grid, 0, "Intervallo tra click aggiuntivi (ms)", &gap);
    let press = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
    press.set_value(config.press_ms.clamp(1, 100) as f64);
    labeled_row(&timing_grid, 1, "Durata della pressione virtuale (ms)", &press);
    let max_hold = gtk::SpinButton::with_range(1.0, 2000.0, 10.0);
    max_hold.set_value(config.max_hold_ms.clamp(1, 2000) as f64);
    labeled_row(&timing_grid, 2, "Soglia trascinamento (ms)", &max_hold);
    root.append(&timing_grid);

    let note = gtk::Label::new(Some("Se la stessa combinazione pulsante/profilo è presente sia in 2x sia in 3x, quando entrambi sono attivi prevale il conteggio 3x per quel pulsante. Gli altri pulsanti mantengono la propria regola. I pulsanti assegnati agli interruttori vengono consumati e non eseguono la loro funzione originale. La GUI resta senza privilegi: l'autorizzazione viene richiesta solo quando avvii il motore."));
    note.set_wrap(true);
    note.set_halign(gtk::Align::Start);
    note.add_css_class("dim-label");
    root.append(&note);

    let status = gtk::Label::new(Some("Configurazione pronta."));
    status.set_halign(gtk::Align::Start);
    status.set_wrap(true);
    root.append(&status);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let save_button = gtk::Button::with_label("Salva configurazione");
    let start_button = gtk::Button::with_label("Avvia ClickForge");
    start_button.add_css_class("suggested-action");
    let stop_button = gtk::Button::with_label("Ferma");
    let log_button = gtk::Button::with_label("Apri log");
    buttons.append(&save_button);
    buttons.append(&start_button);
    buttons.append(&stop_button);
    buttons.append(&log_button);
    root.append(&buttons);

    scroller.set_child(Some(&root));
    window.set_child(Some(&scroller));
    let process: Rc<RefCell<Option<Child>>> = Rc::new(RefCell::new(None));

    // Salvataggio della configurazione multi-pulsante.
    {
        let status = status.clone();
        let settings_path = settings_path.clone();
        let device = device.clone();
        let double_rules = double_rule_widgets.clone();
        let triple_rules = triple_rule_widgets.clone();
        let double_toggle = double_toggle.clone();
        let triple_toggle = triple_toggle.clone();
        let double_enabled = double_enabled.clone();
        let triple_enabled = triple_enabled.clone();
        let osd = osd.clone();
        let gap = gap.clone();
        let press = press.clone();
        let max_hold = max_hold.clone();
        save_button.connect_clicked(move |_| {
            let config = collect_config(
                &device, &double_rules, &triple_rules,
                &double_toggle, &triple_toggle, &double_enabled, &triple_enabled,
                &osd, &gap, &press, &max_hold,
            );
            if let Err(error) = validate_config(&config) {
                status.set_text(&error);
                return;
            }
            match save_config(&settings_path, &config) {
                Ok(()) => status.set_text(&format!("Configurazione salvata in {}", settings_path.display())),
                Err(error) => status.set_text(&format!("Errore durante il salvataggio: {error:#}")),
            }
        });
    }

    // Avvia il motore con pkexec; GTK resta nel processo non privilegiato.
    {
        let status = status.clone();
        let settings_path = settings_path.clone();
        let process = Rc::clone(&process);
        let device = device.clone();
        let double_rules = double_rule_widgets.clone();
        let triple_rules = triple_rule_widgets.clone();
        let double_toggle = double_toggle.clone();
        let triple_toggle = triple_toggle.clone();
        let double_enabled = double_enabled.clone();
        let triple_enabled = triple_enabled.clone();
        let osd = osd.clone();
        let gap = gap.clone();
        let press = press.clone();
        let max_hold = max_hold.clone();
        start_button.connect_clicked(move |_| {
            if process.borrow().is_some() {
                status.set_text("ClickForge risulta già in esecuzione.");
                return;
            }

            let config = collect_config(
                &device, &double_rules, &triple_rules,
                &double_toggle, &triple_toggle, &double_enabled, &triple_enabled,
                &osd, &gap, &press, &max_hold,
            );
            if let Err(error) = validate_config(&config) {
                status.set_text(&error);
                return;
            }
            if let Err(error) = save_config(&settings_path, &config) {
                status.set_text(&format!("Non riesco a salvare la configurazione: {error:#}"));
                return;
            }

            let executable = match std::env::current_exe() {
                Ok(path) => path,
                Err(error) => {
                    status.set_text(&format!("Impossibile individuare ClickForge: {error}"));
                    return;
                }
            };
            let log = match OpenOptions::new().create(true).append(true).open(log_path(&settings_path)) {
                Ok(file) => file,
                Err(error) => {
                    status.set_text(&format!("Impossibile aprire il log: {error}"));
                    return;
                }
            };
            let stderr = match log.try_clone() {
                Ok(file) => file,
                Err(error) => {
                    status.set_text(&format!("Impossibile duplicare il file di log: {error}"));
                    return;
                }
            };

            let mut command = Command::new("pkexec");
            command
                .arg(executable)
                .arg("--double-toggle-button").arg(&config.double_toggle_button)
                .arg("--triple-toggle-button").arg(&config.triple_toggle_button)
                .arg("--double-enabled").arg(config.double_enabled.to_string())
                .arg("--triple-enabled").arg(config.triple_enabled.to_string())
                .arg("--osd").arg(&config.osd)
                .arg("--gap-ms").arg(config.gap_ms.to_string())
                .arg("--press-ms").arg(config.press_ms.to_string())
                .arg("--max-hold-ms").arg(config.max_hold_ms.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(stderr));

            for rule in &config.double_rules {
                command.arg("--double-button").arg(format!("{}:{}", rule.button, rule.clicks));
            }
            for rule in &config.triple_rules {
                command.arg("--triple-button").arg(format!("{}:{}", rule.button, rule.clicks));
            }
            if config.device != "auto" {
                command.arg("--device").arg(&config.device);
            }

            match command.spawn() {
                Ok(child) => {
                    *process.borrow_mut() = Some(child);
                    status.set_text("Richiesta di autorizzazione inviata. Completa il dialogo di sistema per avviare ClickForge.");
                }
                Err(error) => status.set_text(&format!("Impossibile avviare pkexec: {error}. Verifica che polkit sia installato.")),
            }
        });
    }

    // Arresta il processo privilegiato avviato da pkexec.
    {
        let process = Rc::clone(&process);
        let status = status.clone();
        stop_button.connect_clicked(move |_| {
            if let Some(mut child) = process.borrow_mut().take() {
                let _ = child.kill();
                let _ = child.wait();
                status.set_text("Richiesta di arresto inviata a ClickForge.");
            } else {
                status.set_text("ClickForge non risulta in esecuzione.");
            }
        });
    }

    {
        let settings_path = settings_path.clone();
        let status = status.clone();
        log_button.connect_clicked(move |_| {
            let path = log_path(&settings_path);
            match Command::new("xdg-open").arg(&path).spawn() {
                Ok(_) => status.set_text(&format!("Apertura del log: {}", path.display())),
                Err(error) => status.set_text(&format!("Impossibile aprire il log: {error}")),
            }
        });
    }

    // Aggiorna lo stato quando pkexec/il motore termina o l'autenticazione viene annullata.
    {
        let process = Rc::clone(&process);
        let status = status.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(600), move || {
            let completed = {
                let mut slot = process.borrow_mut();
                let poll_result = slot.as_mut().map(|child| child.try_wait());
                match poll_result {
                    Some(Ok(Some(exit))) => {
                        slot.take();
                        Some(format!("ClickForge è terminato ({exit}). Controlla il log per i dettagli."))
                    }
                    Some(Ok(None)) | None => None,
                    Some(Err(error)) => {
                        slot.take();
                        Some(format!("Errore controllando ClickForge: {error}"))
                    }
                }
            };
            if let Some(message) = completed {
                status.set_text(&message);
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    if let Some(warning) = load_warning {
        status.set_text(&warning);
    }

    window.present();
}
