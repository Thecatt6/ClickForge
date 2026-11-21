use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, Device, EventType, InputEvent, KeyCode, MiscCode, PropType,
    RelativeAxisCode,
};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
    Arc,
    Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

mod gui;

#[derive(Debug, Parser)]
#[command(
    name = "clickforge",
    version,
    about = "Moltiplicatore di click per Linux con GUI opzionale e profili multi-pulsante",
    long_about = "ClickForge moltiplica i click di più pulsanti del mouse. Ogni profilo può avere regole indipendenti. Ripeti --double-button o --triple-button per assegnare più pulsanti, usando il formato BUTTON:COUNT."
)]
struct Args {
    /// Apre l'interfaccia grafica di configurazione (non richiede root)
    #[arg(long)]
    gui: bool,

    /// Elenca i dispositivi input e termina
    #[arg(long)]
    list: bool,

    /// Dispositivo da usare. Se omesso, ClickForge rileva automaticamente il mouse.
    #[arg(long, value_name = "PATH", help = "Percorso /dev/input/eventX; default: rilevamento automatico")]
    device: Option<PathBuf>,

    /// Regola del profilo 2x, ripetibile: --double-button left:2 --double-button right:2
    #[arg(long = "double-button", value_name = "BUTTON:COUNT", action = clap::ArgAction::Append)]
    double_buttons: Vec<String>,

    /// Regola del profilo 3x, ripetibile: --triple-button left:3 --triple-button right:5
    #[arg(long = "triple-button", value_name = "BUTTON:COUNT", action = clap::ArgAction::Append)]
    triple_buttons: Vec<String>,

    /// Compatibilità con i comandi precedenti: pulsante comune ai profili 2x/3x
    #[arg(long = "click-button", value_enum)]
    legacy_click_button: Option<MouseButton>,

    /// Pulsante che attiva/disattiva il profilo 2x (default: back)
    #[arg(long, value_enum, default_value = "back")]
    double_toggle_button: ToggleButton,

    /// Pulsante che attiva/disattiva il profilo 3x (default: forward)
    #[arg(long, value_enum, default_value = "forward")]
    triple_toggle_button: ToggleButton,

    /// Conteggio legacy per --click-button nel profilo 2x (1-20)
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8))]
    double_clicks: u8,

    /// Conteggio legacy per --click-button nel profilo 3x (1-20)
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8))]
    triple_clicks: u8,

    /// Stato iniziale del profilo associato al toggle posteriore
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    double_enabled: bool,

    /// Stato iniziale del profilo associato al toggle anteriore
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    triple_enabled: bool,

    /// Visualizzazione dello stato: overlay (Wayland), notification oppure none
    #[arg(long, value_enum, default_value = "overlay")]
    osd: OsdMode,

    /// Pausa tra il click originale e ogni click aggiuntivo, in millisecondi
    #[arg(long, default_value_t = 45, value_parser = clap::value_parser!(u64).range(0..=1000))]
    gap_ms: u64,

    /// Durata della pressione dei click generati, in millisecondi
    #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u64).range(1..=100))]
    press_ms: u64,

    /// Le pressioni più lunghe sono considerate trascinamenti, non click
    #[arg(long, default_value_t = 250, value_parser = clap::value_parser!(u64).range(1..=2000))]
    max_hold_ms: u64,

    /// Modalità interna usata dal processo OSD (non usare manualmente)
    #[arg(long, hide = true)]
    osd_server: bool,

    /// File in cui scrivere il PID del motore (la GUI lo usa per inviare SIGTERM e fermarlo
    /// in modo pulito invece di uccidere pkexec)
    #[arg(long, value_name = "PATH")]
    pidfile: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl MouseButton {
    fn key_code(self) -> KeyCode {
        match self {
            Self::Left => KeyCode::BTN_LEFT,
            Self::Right => KeyCode::BTN_RIGHT,
            Self::Middle => KeyCode::BTN_MIDDLE,
            Self::Back => KeyCode::BTN_SIDE,
            Self::Forward => KeyCode::BTN_EXTRA,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Left => "sinistro",
            Self::Right => "destro",
            Self::Middle => "centrale",
            Self::Back => "laterale posteriore",
            Self::Forward => "laterale anteriore",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum ToggleButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    None,
}

impl ToggleButton {
    fn key_code(self) -> Option<KeyCode> {
        match self {
            Self::Left => Some(KeyCode::BTN_LEFT),
            Self::Right => Some(KeyCode::BTN_RIGHT),
            Self::Middle => Some(KeyCode::BTN_MIDDLE),
            Self::Back => Some(KeyCode::BTN_SIDE),
            Self::Forward => Some(KeyCode::BTN_EXTRA),
            Self::None => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Left => "sinistro",
            Self::Right => "destro",
            Self::Middle => "centrale",
            Self::Back => "laterale posteriore",
            Self::Forward => "laterale anteriore",
            Self::None => "nessuno",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum OsdMode {
    Overlay,
    Notification,
    None,
}

#[derive(Clone, Copy, Debug)]
struct ClickRule {
    button: MouseButton,
    clicks: u8,
}

impl ClickRule {
    fn key(self) -> KeyCode {
        self.button.key_code()
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Errore: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();

    if args.gui {
        if unsafe { libc::geteuid() } == 0 {
            bail!("avvia la GUI senza sudo: sarà pkexec a chiedere i permessi quando premi Avvia");
        }
        return gui::run_gui();
    }

    if args.osd_server {
        return run_overlay_server();
    }
    if args.list {
        list_devices();
        return Ok(());
    }

    let (double_rules, triple_rules) = build_rules(&args)?;
    validate_rules(&double_rules, &triple_rules, args.double_toggle_button, args.triple_toggle_button)?;

    let double_toggle_key = args.double_toggle_button.key_code();
    let triple_toggle_key = args.triple_toggle_button.key_code();
    let mut required_keys: Vec<KeyCode> = double_rules
        .iter()
        .chain(triple_rules.iter())
        .map(|rule| rule.key())
        .collect();
    for key in [double_toggle_key, triple_toggle_key].into_iter().flatten() {
        if !required_keys.contains(&key) {
            required_keys.push(key);
        }
    }

    let (device_path, mut physical) = match args.device.as_deref() {
        None => find_mouse_automatically(&required_keys)?,
        Some(path) if path == Path::new("auto") => find_mouse_automatically(&required_keys)?,
        Some(path) => {
            let device = Device::open(path).with_context(|| {
                format!("impossibile aprire {} (prova ad avviare con sudo)", path.display())
            })?;
            (path.to_path_buf(), device)
        }
    };

    if !is_mouse_candidate(&physical) {
        bail!(
            "{} non sembra un mouse relativo: deve supportare BTN_LEFT, REL_X e REL_Y. Usa --list per trovare il dispositivo corretto",
            device_path.display()
        );
    }

    let keys: AttributeSet<KeyCode> = physical
        .supported_keys()
        .map(|set| set.iter().collect())
        .unwrap_or_default();
    for (rule, profile) in double_rules.iter().map(|r| (r, "2x")).chain(triple_rules.iter().map(|r| (r, "3x"))) {
        if !keys.contains(rule.key()) {
            bail!(
                "il mouse selezionato non espone il pulsante {} assegnato al profilo {profile}; usa --list o seleziona un altro dispositivo",
                rule.button.label()
            );
        }
    }
    for (label, key) in [
        ("interruttore 2x", double_toggle_key),
        ("interruttore 3x", triple_toggle_key),
    ] {
        if let Some(key) = key {
            if !keys.contains(key) {
                bail!("il mouse selezionato non espone il pulsante assegnato a {label}");
            }
        }
    }

    let relative_axes: AttributeSet<RelativeAxisCode> = physical
        .supported_relative_axes()
        .map(|set| set.iter().collect())
        .unwrap_or_default();
    let misc_codes: AttributeSet<MiscCode> = physical
        .misc_properties()
        .map(|set| set.iter().collect())
        .unwrap_or_default();
    let properties: AttributeSet<PropType> = physical.properties().iter().collect();

    let builder = VirtualDevice::builder()?
        .name("ClickForge Virtual Mouse")
        .with_keys(&keys)?
        .with_relative_axes(&relative_axes)?
        .with_msc(&misc_codes)?
        .with_properties(&properties)?;
    let virtual_mouse = builder
        .build()
        .context("impossibile creare il mouse virtuale (controlla /dev/uinput)")?;

    physical
        .set_nonblocking(true)
        .context("impossibile impostare la lettura non bloccante")?;
    physical
        .grab()
        .context("impossibile acquisire il mouse in esclusiva")?;

    // Rilascia i privilegi il prima possibile: i descrittori dei dispositivi restano
    // validi e il ciclo di elaborazione successivo NON gira più come root. L'ambiente
    // (Wayland, D-Bus, HOME) viene riallineato alla sessione grafica dell'utente reale,
    // così overlay e notifiche continuano a funzionare senza runuser.
    if drop_privileges_to_invoking_user() {
        println!("Privilegi di root rilasciati: ClickForge continua come utente non privilegiato.");
    }

    let running = Arc::new(AtomicBool::new(true));
    let signal_flag = Arc::clone(&running);
    // Gestisce SIGINT (Ctrl+C), SIGTERM e SIGHUP: in tutti i casi il motore
    // termina in modo pulito e rilascia il mouse fisico.
    ctrlc::set_handler(move || signal_flag.store(false, Ordering::SeqCst))
        .context("impossibile installare il gestore dei segnali")?;

    // PID file: permette alla GUI di individuare il motore e fermarlo con SIGTERM.
    let _pid_guard = match args.pidfile.as_deref() {
        Some(path) => Some(write_pid_file(path)?),
        None => None,
    };

    let virtual_mouse = Arc::new(Mutex::new(virtual_mouse));
    let (emitter_tx, emitter_handle) =
        start_click_emitter(Arc::clone(&virtual_mouse), Arc::clone(&running));

    let mut overlay_stdin: Option<ChildStdin> = match args.osd {
        OsdMode::Overlay => match start_overlay_helper() {
            Ok(stdin) => Some(stdin),
            Err(error) => {
                eprintln!("OSD overlay non avviato: {error:#}");
                eprintln!("Per usare le notifiche KDE, riavvia con --osd notification; per disattivare l'OSD usa --osd none.");
                None
            }
        },
        _ => None,
    };

    let physical_name = physical.name().unwrap_or("mouse senza nome");
    println!("ClickForge {} avviato", env!("CARGO_PKG_VERSION"));
    println!("Mouse: {} ({})", physical_name, device_path.display());
    println!("Regole 2x: {}", describe_rules(&double_rules));
    println!("Regole 3x: {}", describe_rules(&triple_rules));
    println!("Interruttore 2x: {} · interruttore 3x: {}", args.double_toggle_button.label(), args.triple_toggle_button.label());
    println!("OSD: {:?}", args.osd);
    println!("Premi i pulsanti assegnati per attivare/disattivare i profili; Ctrl+C per uscire.");

    let mut double_enabled = args.double_enabled && args.double_toggle_button != ToggleButton::None;
    let mut triple_enabled = args.triple_enabled && args.triple_toggle_button != ToggleButton::None;
    println!(
        "Stato iniziale: 2x {} · 3x {}\n",
        if double_enabled { "ATTIVO" } else { "DISATTIVO" },
        if triple_enabled { "ATTIVO" } else { "DISATTIVO" }
    );
    if let Some(stdin) = overlay_stdin.as_mut() {
        let _ = send_overlay_state(stdin, double_enabled, triple_enabled, &double_rules, &triple_rules);
    }

    let mut frame: Vec<InputEvent> = Vec::with_capacity(12);
    // Per ogni pulsante teniamo traccia del conteggio scelto al momento della pressione.
    let mut pressed_clicks: HashMap<u16, (Instant, u8)> = HashMap::new();
    let mut double_toggle_is_down = false;
    let mut triple_toggle_is_down = false;
    let max_hold = Duration::from_millis(args.max_hold_ms);
    let mut osd_warning_shown = false;

    while running.load(Ordering::SeqCst) {
        // Aspetta l'arrivo di eventi sul descrittore (max 2 ms) invece di dormire a vuoto:
        // latenza minima e nessun consumo di CPU quando il mouse è fermo.
        let mut poll_fd = libc::pollfd {
            fd: physical.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll_fd, 1, 2) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                // Segnale ricevuto: ricontrolla `running` ed esci se richiesto.
                continue;
            }
            return Err(error).context("errore durante l'attesa degli eventi del mouse");
        }
        if poll_fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            bail!("il dispositivo di input non è più disponibile (mouse scollegato?)");
        }
        if ready == 0 {
            continue;
        }

        let incoming: Vec<InputEvent> = match physical.fetch_events() {
            Ok(events) => events.collect(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error).context("errore durante la lettura del mouse"),
        };

        if incoming.is_empty() {
            continue;
        }

        for event in incoming {
            if event.event_type() == EventType::SYNCHRONIZATION {
                if frame.is_empty() {
                    continue;
                }

                let mut forwarded_frame: Vec<InputEvent> = Vec::with_capacity(frame.len());
                let mut extra_clicks: Vec<(KeyCode, u8)> = Vec::with_capacity(4);

                for item in frame.drain(..) {
                    if item.event_type() == EventType::KEY {
                        let key = KeyCode(item.code());

                        // Il toggle è consumato e non raggiunge le applicazioni.
                        if double_toggle_key.is_some_and(|code| item.code() == code.0) {
                            match item.value() {
                                1 if !double_toggle_is_down => {
                                    double_toggle_is_down = true;
                                    double_enabled = !double_enabled;
                                    println!("ClickForge: profilo 2x {}", if double_enabled { "ATTIVO" } else { "DISATTIVO" });
                                    show_osd_state(args.osd, overlay_stdin.as_mut(), double_enabled, triple_enabled, &double_rules, &triple_rules, &mut osd_warning_shown);
                                }
                                0 => double_toggle_is_down = false,
                                _ => {}
                            }
                            continue;
                        }

                        if triple_toggle_key.is_some_and(|code| item.code() == code.0) {
                            match item.value() {
                                1 if !triple_toggle_is_down => {
                                    triple_toggle_is_down = true;
                                    triple_enabled = !triple_enabled;
                                    println!("ClickForge: profilo 3x {}", if triple_enabled { "ATTIVO" } else { "DISATTIVO" });
                                    show_osd_state(args.osd, overlay_stdin.as_mut(), double_enabled, triple_enabled, &double_rules, &triple_rules, &mut osd_warning_shown);
                                }
                                0 => triple_toggle_is_down = false,
                                _ => {}
                            }
                            continue;
                        }

                        // Se la stessa combinazione compare in entrambi i profili,
                        // il 3x prevale. Gli altri pulsanti mantengono la propria regola.
                        let selected_count = select_click_count(
                            key,
                            &double_rules,
                            &triple_rules,
                            double_enabled,
                            triple_enabled,
                        );

                        match item.value() {
                            1 => {
                                if let Some(total_clicks) = selected_count {
                                    pressed_clicks.entry(item.code()).or_insert((Instant::now(), total_clicks));
                                }
                            }
                            0 => {
                                if let Some((started, total_clicks)) = pressed_clicks.remove(&item.code()) {
                                    if started.elapsed() <= max_hold && total_clicks > 1 {
                                        // Il click fisico originale viene inoltrato; aggiungiamo solo i mancanti.
                                        extra_clicks.push((key, total_clicks - 1));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    forwarded_frame.push(item);
                }

                if !forwarded_frame.is_empty() {
                    let mut device = virtual_mouse.lock().unwrap_or_else(|error| error.into_inner());
                    device
                        .emit(&forwarded_frame)
                        .context("impossibile inoltrare un frame al mouse virtuale")?;
                }

                // I click aggiuntivi vengono accodati al thread dedicato: il loop di
                // lettura non si ferma mai, quindi il puntatore resta fluido anche
                // mentre i click extra vengono generati.
                for (button, count) in extra_clicks {
                    let batch = ExtraClicks {
                        button,
                        count,
                        gap_ms: args.gap_ms,
                        press_ms: args.press_ms,
                    };
                    if emitter_tx.send(batch).is_err() {
                        eprintln!("ClickForge: il thread dei click aggiuntivi non risponde; click extra ignorati.");
                    }
                }
            } else if matches!(event.event_type(), EventType::KEY | EventType::RELATIVE | EventType::MISC) {
                frame.push(event);
            }
        }
    }

    // Chiudi prima il canale dei click aggiuntivi e lascia terminare il thread
    // (esce entro press_ms), poi rilascia il mouse fisico.
    drop(emitter_tx);
    let _ = emitter_handle.join();
    let _ = physical.ungrab();
    drop(overlay_stdin);
    println!("\nClickForge fermato.");
    Ok(())
}

fn build_rules(args: &Args) -> Result<(Vec<ClickRule>, Vec<ClickRule>)> {
    if !(1..=20).contains(&args.double_clicks) || !(1..=20).contains(&args.triple_clicks) {
        bail!("i conteggi legacy devono essere compresi tra 1 e 20");
    }

    if args.double_buttons.is_empty() && args.triple_buttons.is_empty() {
        let button = args.legacy_click_button.unwrap_or(MouseButton::Left);
        return Ok((
            vec![ClickRule { button, clicks: args.double_clicks }],
            vec![ClickRule { button, clicks: args.triple_clicks }],
        ));
    }

    Ok((parse_rules(&args.double_buttons, "2x")?, parse_rules(&args.triple_buttons, "3x")?))
}

fn parse_rules(values: &[String], profile: &str) -> Result<Vec<ClickRule>> {
    let mut rules = Vec::new();
    for value in values {
        let (button_text, clicks_text) = value.split_once(':').with_context(|| {
            format!("regola `{value}` non valida per il profilo {profile}: usa BUTTON:COUNT, ad esempio left:2")
        })?;
        let button = parse_mouse_button(button_text)?;
        let clicks: u8 = clicks_text.parse().with_context(|| format!("conteggio non valido nella regola `{value}`"))?;
        if !(1..=20).contains(&clicks) {
            bail!("il conteggio nella regola `{value}` deve essere compreso tra 1 e 20");
        }
        if rules.iter().any(|rule: &ClickRule| rule.button == button) {
            bail!("il pulsante {} è assegnato più di una volta al profilo {profile}", button.label());
        }
        rules.push(ClickRule { button, clicks });
    }
    Ok(rules)
}

fn parse_mouse_button(value: &str) -> Result<MouseButton> {
    match value.to_ascii_lowercase().as_str() {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        "back" => Ok(MouseButton::Back),
        "forward" => Ok(MouseButton::Forward),
        _ => bail!("pulsante `{value}` non valido: usa left, right, middle, back oppure forward"),
    }
}

fn validate_rules(
    double_rules: &[ClickRule],
    triple_rules: &[ClickRule],
    double_toggle: ToggleButton,
    triple_toggle: ToggleButton,
) -> Result<()> {
    if double_rules.is_empty() && triple_rules.is_empty() {
        bail!("assegna almeno un pulsante da moltiplicare in uno dei due profili");
    }
    if double_toggle != ToggleButton::None && double_toggle == triple_toggle {
        bail!("gli interruttori dei profili 2x e 3x devono essere diversi");
    }

    let toggle_keys: Vec<KeyCode> = [double_toggle.key_code(), triple_toggle.key_code()]
        .into_iter()
        .flatten()
        .collect();
    for rule in double_rules.iter().chain(triple_rules.iter()) {
        if toggle_keys.contains(&rule.key()) {
            bail!(
                "il pulsante {} non può essere sia un pulsante da moltiplicare sia un interruttore di profilo",
                rule.button.label()
            );
        }
    }
    Ok(())
}

fn find_rule_count(rules: &[ClickRule], key: KeyCode) -> Option<u8> {
    rules.iter().find(|rule| rule.key() == key).map(|rule| rule.clicks)
}

fn select_click_count(
    key: KeyCode,
    double_rules: &[ClickRule],
    triple_rules: &[ClickRule],
    double_enabled: bool,
    triple_enabled: bool,
) -> Option<u8> {
    if triple_enabled {
        find_rule_count(triple_rules, key).or_else(|| {
            if double_enabled { find_rule_count(double_rules, key) } else { None }
        })
    } else if double_enabled {
        find_rule_count(double_rules, key)
    } else {
        None
    }
}

fn describe_rules(rules: &[ClickRule]) -> String {
    if rules.is_empty() {
        return "nessun pulsante assegnato".to_string();
    }
    rules
        .iter()
        .map(|rule| format!("{} ×{}", rule.button.label(), rule.clicks))
        .collect::<Vec<_>>()
        .join(", ")
}

fn is_mouse_candidate(device: &Device) -> bool {
    let has_left_button = device
        .supported_keys()
        .is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT));
    let has_relative_xy = device.supported_relative_axes().is_some_and(|axes| {
        axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y)
    });
    has_left_button && has_relative_xy
}

fn supports_key(device: &Device, key: KeyCode) -> bool {
    device.supported_keys().is_some_and(|keys| keys.contains(key))
}

fn find_mouse_automatically(required_keys: &[KeyCode]) -> Result<(PathBuf, Device)> {
    let mut candidates: Vec<(i32, PathBuf, Device)> = evdev::enumerate()
        .filter_map(|(path, device)| {
            if !is_mouse_candidate(&device)
                || device.name().is_some_and(|name| name.contains("ClickForge Virtual Mouse"))
                || !required_keys.iter().all(|key| supports_key(&device, *key))
            {
                return None;
            }
            Some((device_score(&device), path, device))
        })
        .collect();

    if candidates.is_empty() {
        bail!("non ho trovato automaticamente un mouse che supporti tutti i pulsanti configurati. Prova `sudo target/release/clickforge --list`, quindi specifica --device /dev/input/eventX");
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    if candidates.len() > 1 {
        eprintln!("Trovati {} dispositivi compatibili; scelgo il candidato con più capacità mouse:", candidates.len());
        for (index, (score, path, device)) in candidates.iter().enumerate() {
            eprintln!("  {}. {} — {} (punteggio {}){}", index + 1, path.display(), device.name().unwrap_or("mouse senza nome"), score, if index == 0 { " [selezionato]" } else { "" });
        }
    }

    let (_, path, device) = candidates.remove(0);
    println!("Mouse rilevato automaticamente: {} ({})", device.name().unwrap_or("mouse senza nome"), path.display());
    Ok((path, device))
}

fn device_score(device: &Device) -> i32 {
    let mut score = 10;
    if supports_key(device, KeyCode::BTN_SIDE) { score += 5; }
    if supports_key(device, KeyCode::BTN_EXTRA) { score += 5; }
    if device.supported_relative_axes().is_some_and(|axes| axes.contains(RelativeAxisCode::REL_WHEEL)) { score += 2; }
    let name = device.name().unwrap_or("").to_ascii_lowercase();
    if name.contains("mouse") || name.contains("logitech") || name.contains("razer") || name.contains("steelseries") { score += 2; }
    score
}

fn list_devices() {
    let mut count = 0usize;
    println!("Dispositivi input rilevati:\n");
    for (path, device) in evdev::enumerate() {
        count += 1;
        let marker = if is_mouse_candidate(&device) { "MOUSE" } else { "altro" };
        let side = supports_key(&device, KeyCode::BTN_SIDE);
        let forward = supports_key(&device, KeyCode::BTN_EXTRA);
        println!("{:<26} [{:<5}] {:<36} laterale-back={} laterale-forward={}", path.display(), marker, device.name().unwrap_or("dispositivo senza nome"), if side { "si" } else { "no" }, if forward { "si" } else { "no" });
    }
    if count == 0 { println!("Nessun dispositivo trovato. Prova a eseguire il comando con sudo."); }
}

/// Un lotto di click aggiuntivi generato dalla pressione di un pulsante fisico.
struct ExtraClicks {
    button: KeyCode,
    count: u8,
    gap_ms: u64,
    press_ms: u64,
}

/// Avvia il thread dedicato ai click aggiuntivi. Emette i click con la stessa cadenza
/// di prima (gap_ms prima di ogni click, pressione di press_ms), ma senza bloccare il
/// loop principale: movimento del puntatore ed eventi del mouse fisico non vengono
/// più accumulati in attesa che i click extra finiscano.
fn start_click_emitter(
    virtual_mouse: Arc<Mutex<VirtualDevice>>,
    running: Arc<AtomicBool>,
) -> (mpsc::Sender<ExtraClicks>, thread::JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel::<ExtraClicks>();
    let handle = thread::spawn(move || {
        while let Ok(batch) = receiver.recv() {
            for _ in 0..batch.count {
                if !running.load(Ordering::SeqCst) {
                    return;
                }
                if batch.gap_ms > 0 {
                    thread::sleep(Duration::from_millis(batch.gap_ms));
                }
                if !running.load(Ordering::SeqCst) {
                    return;
                }
                {
                    let mut device = virtual_mouse.lock().unwrap_or_else(|error| error.into_inner());
                    if let Err(error) = device.emit(&[InputEvent::new(EventType::KEY.0, batch.button.0, 1)]) {
                        eprintln!("ClickForge: impossibile generare la pressione virtuale: {error}");
                        return;
                    }
                }
                thread::sleep(Duration::from_millis(batch.press_ms));
                {
                    let mut device = virtual_mouse.lock().unwrap_or_else(|error| error.into_inner());
                    if let Err(error) = device.emit(&[InputEvent::new(EventType::KEY.0, batch.button.0, 0)]) {
                        eprintln!("ClickForge: impossibile generare il rilascio virtuale: {error}");
                        return;
                    }
                }
            }
        }
    });
    (sender, handle)
}

/// Rimuove il pid file quando il processo termina (anche su errore o Ctrl+C).
struct PidFileGuard(PathBuf);

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_pid_file(path: &Path) -> Result<PidFileGuard> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("impossibile creare la cartella {}", parent.display()))?;
    }
    std::fs::write(path, format!("{}\n", std::process::id()))
        .with_context(|| format!("impossibile scrivere il pid file {}", path.display()))?;
    Ok(PidFileGuard(path.to_path_buf()))
}

/// UID dell'utente che ha invocato il comando tramite pkexec o sudo, se presente.
fn invoking_uid() -> Option<u32> {
    ["PKEXEC_UID", "SUDO_UID"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find_map(|value| value.trim().parse::<u32>().ok())
}

/// Se il processo è root avviato tramite pkexec/sudo, riallinea l'ambiente alla
/// sessione grafica dell'utente reale (Wayland, D-Bus, HOME) e rilascia i privilegi.
/// Ritorna true se il rilascio è avvenuto. In caso di fallimento il motore continua
/// come root e tutto funziona come prima.
fn drop_privileges_to_invoking_user() -> bool {
    if unsafe { libc::geteuid() } != 0 {
        return false;
    }
    let uid = match invoking_uid() {
        Some(uid) => uid,
        None => return false,
    };

    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0u8; 16384];
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buffer.as_mut_ptr() as *mut libc::c_char,
            buffer.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        eprintln!("ClickForge: impossibile risolvere i dati dell'utente (UID {uid}); continuo come root.");
        return false;
    }
    let gid = pwd.pw_gid;
    if pwd.pw_dir.is_null() || pwd.pw_name.is_null() {
        eprintln!("ClickForge: voce passwd incompleta per UID {uid}; continuo come root.");
        return false;
    }
    let home = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) }
        .to_string_lossy()
        .into_owned();
    let username = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }
        .to_string_lossy()
        .into_owned();

    let runtime_dir = format!("/run/user/{uid}");
    if Path::new(&runtime_dir).is_dir() {
        std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
        let bus = format!("{runtime_dir}/bus");
        if Path::new(&bus).exists() && std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={bus}"));
        }
        let wayland_ok = std::env::var("WAYLAND_DISPLAY")
            .ok()
            .map(|display| Path::new(&runtime_dir).join(&display).exists())
            .unwrap_or(false);
        if !wayland_ok {
            match find_wayland_socket(&runtime_dir) {
                Some(display) => std::env::set_var("WAYLAND_DISPLAY", display),
                // WAYLAND_DISPLAY obsoleto o assente: rimuovilo per non puntare a un socket inesistente.
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
        }
    }
    std::env::set_var("HOME", &home);
    std::env::set_var("USER", &username);
    std::env::set_var("LOGNAME", &username);

    if unsafe { libc::setgroups(0, std::ptr::null()) } != 0 {
        eprintln!("ClickForge: impossibile azzerare i gruppi supplementari; continuo comunque.");
    }
    if unsafe { libc::setgid(gid) } != 0 || unsafe { libc::setuid(uid) } != 0 {
        eprintln!("ClickForge: impossibile rilasciare i privilegi di root; continuo come root.");
        return false;
    }
    true
}

fn start_overlay_helper() -> Result<ChildStdin> {
    let executable = std::env::current_exe().context("impossibile individuare l'eseguibile ClickForge")?;
    let mut command;

    if unsafe { libc::geteuid() } == 0 {
        // Supporta sia `sudo clickforge` sia l'avvio dalla GUI tramite `pkexec`.
        let (uid, username) = if let (Ok(username), Ok(uid)) = (
            std::env::var("SUDO_USER"),
            std::env::var("SUDO_UID"),
        ) {
            (uid, username)
        } else {
            let uid = std::env::var("PKEXEC_UID")
                .context("ClickForge è root ma non riesce a identificare l'utente della sessione (SUDO_UID/PKEXEC_UID assente)")?;
            let passwd = Command::new("getent")
                .arg("passwd")
                .arg(&uid)
                .output()
                .context("impossibile risolvere PKEXEC_UID con getent")?;
            if !passwd.status.success() {
                bail!("impossibile trovare l'utente associato a UID {uid}");
            }
            let entry = String::from_utf8_lossy(&passwd.stdout);
            let username = entry
                .trim()
                .split(':')
                .next()
                .filter(|name| !name.is_empty())
                .context("voce passwd non valida per PKEXEC_UID")?
                .to_string();
            (uid, username)
        };

        let runtime_dir = format!("/run/user/{uid}");
        let wayland_display = std::env::var("WAYLAND_DISPLAY")
            .ok()
            .or_else(|| find_wayland_socket(&runtime_dir))
            .context("socket Wayland non trovato nella sessione dell'utente")?;
        let bus_path = format!("{runtime_dir}/bus");

        command = Command::new("runuser");
        command
            .arg("-u").arg(username).arg("--").arg("env")
            .arg(format!("XDG_RUNTIME_DIR={runtime_dir}"))
            .arg(format!("DBUS_SESSION_BUS_ADDRESS=unix:path={bus_path}"))
            .arg(format!("WAYLAND_DISPLAY={wayland_display}"))
            .arg("GDK_BACKEND=wayland")
            .arg("PATH=/usr/bin:/bin")
            .arg(executable)
            .arg("--osd-server");
    } else {
        command = Command::new(executable);
        command.arg("--osd-server");
        // L'overlay richiede il backend Wayland (Layer Shell): forzalo quando
        // il socket Wayland è noto, altrimenti GTK potrebbe scegliere X11.
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            command.env("GDK_BACKEND", "wayland");
        }
    }

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("impossibile avviare il processo overlay (controlla GTK4 e gtk4-layer-shell)")?;
    child.stdin.take().context("impossibile aprire il canale con l'overlay")
}

fn find_wayland_socket(runtime_dir: &str) -> Option<String> {
    let mut sockets: Vec<String> = std::fs::read_dir(runtime_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if name.starts_with("wayland-") { Some(name) } else { None }
        })
        .collect();
    sockets.sort();
    sockets.into_iter().next()
}

fn describe_modes(
    double_enabled: bool,
    triple_enabled: bool,
    double_rules: &[ClickRule],
    triple_rules: &[ClickRule],
) -> String {
    let double_state = if double_enabled { "ON" } else { "OFF" };
    let triple_state = if triple_enabled { "ON" } else { "OFF" };
    format!(
        "2x {double_state}: {} · 3x {triple_state}: {}",
        describe_rules(double_rules),
        describe_rules(triple_rules)
    )
}

fn send_overlay_state(
    stdin: &mut ChildStdin,
    double_enabled: bool,
    triple_enabled: bool,
    double_rules: &[ClickRule],
    triple_rules: &[ClickRule],
) -> bool {
    let double_summary = describe_rules(double_rules);
    let triple_summary = describe_rules(triple_rules);
    writeln!(
        stdin,
        "{}|{}|{}|{}",
        if double_enabled { "1" } else { "0" },
        if triple_enabled { "1" } else { "0" },
        double_summary,
        triple_summary
    )
    .and_then(|_| stdin.flush())
    .is_ok()
}

fn show_osd_state(
    osd: OsdMode,
    overlay_stdin: Option<&mut ChildStdin>,
    double_enabled: bool,
    triple_enabled: bool,
    double_rules: &[ClickRule],
    triple_rules: &[ClickRule],
    warning_shown: &mut bool,
) {
    match osd {
        OsdMode::Overlay => {
            if let Some(stdin) = overlay_stdin {
                if !send_overlay_state(stdin, double_enabled, triple_enabled, double_rules, triple_rules) && !*warning_shown {
                    eprintln!("OSD overlay: il processo grafico non risponde. Verifica la sessione Wayland e gtk4-layer-shell.");
                    *warning_shown = true;
                }
            }
        }
        OsdMode::Notification => {
            if !show_status_notification(double_enabled, triple_enabled, double_rules, triple_rules) && !*warning_shown {
                eprintln!("Notifiche non disponibili: installa libnotify e verifica le notifiche KDE.");
                *warning_shown = true;
            }
        }
        OsdMode::None => {}
    }
}

fn run_overlay_server() -> Result<()> {
    use gtk::prelude::*;
    use gtk::gdk::prelude::SurfaceExt;
    use gtk::glib;
    use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
    use std::cell::RefCell;
    use std::rc::Rc;

    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in BufReader::new(stdin.lock()).lines() {
            match line {
                Ok(line) => if sender.send(line).is_err() { return; },
                Err(_) => break,
            }
        }
        let _ = sender.send("__QUIT__".to_string());
    });

    let receiver = Rc::new(RefCell::new(receiver));
    let app = gtk::Application::builder().application_id("io.clickforge.overlay").build();
    app.connect_activate(move |app| {
        if !gtk4_layer_shell::is_supported() {
            eprintln!("ClickForge overlay: il compositor corrente non supporta Layer Shell. Usa KDE Wayland oppure --osd notification.");
            app.quit();
            return;
        }

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("ClickForge OSD")
            .decorated(false)
            .resizable(false)
            .default_width(580)
            .default_height(92)
            .build();
        window.set_focusable(false);
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Top, true);
        window.set_margin(Edge::Top, 34);
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(KeyboardMode::None);

        let outer = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        outer.add_css_class("clickforge-toast");
        outer.set_margin_start(18);
        outer.set_margin_end(18);
        outer.set_margin_top(14);
        outer.set_margin_bottom(14);
        outer.set_can_target(false);

        let icon = gtk::Label::new(Some("🖱️"));
        icon.add_css_class("clickforge-icon");
        icon.set_can_target(false);
        let text_box = gtk::Box::new(gtk::Orientation::Vertical, 3);
        text_box.set_valign(gtk::Align::Center);
        text_box.set_can_target(false);
        let headline = gtk::Label::new(Some("ClickForge ATTIVO"));
        headline.add_css_class("clickforge-headline");
        headline.set_halign(gtk::Align::Start);
        headline.set_can_target(false);
        let details = gtk::Label::new(Some("Profili pronti"));
        details.add_css_class("clickforge-details");
        details.set_halign(gtk::Align::Start);
        details.set_wrap(true);
        details.set_max_width_chars(74);
        details.set_can_target(false);

        text_box.append(&headline);
        text_box.append(&details);
        outer.append(&icon);
        outer.append(&text_box);
        window.set_child(Some(&outer));

        let css = gtk::CssProvider::new();
        css.load_from_data(
            "window { background: transparent; }\
             .clickforge-toast { background: rgba(28, 30, 36, 0.96); border: 1px solid rgba(255,255,255,0.16); border-radius: 18px; padding: 8px 22px; }\
             .clickforge-icon { font-size: 28px; color: white; }\
             .clickforge-headline { color: #ffffff; font-size: 17px; font-weight: 800; }\
             .clickforge-details { color: #d1d5db; font-size: 12px; }\
             .clickforge-on { color: #72e5a4; }\
             .clickforge-off { color: #ff9a9a; }",
        );
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }

        let receiver = Rc::clone(&receiver);
        let last_shown: Rc<RefCell<Option<Instant>>> = Rc::new(RefCell::new(None));
        let last_shown_timer = Rc::clone(&last_shown);
        let window_timer = window.clone();
        let headline_timer = headline.clone();
        let details_timer = details.clone();

        glib::timeout_add_local(Duration::from_millis(35), move || {
            let mut should_quit = false;
            loop {
                let message = receiver.borrow_mut().try_recv();
                match message {
                    Ok(line) if line == "__QUIT__" => { should_quit = true; break; }
                    Ok(line) => {
                        let parts: Vec<&str> = line.splitn(4, '|').collect();
                        if parts.len() != 4 { continue; }
                        let double_enabled = parts[0] == "1";
                        let triple_enabled = parts[1] == "1";
                        let any_enabled = double_enabled || triple_enabled;
                        if any_enabled {
                            headline_timer.set_label("ClickForge ATTIVO");
                            headline_timer.remove_css_class("clickforge-off");
                            headline_timer.add_css_class("clickforge-on");
                        } else {
                            headline_timer.set_label("ClickForge DISATTIVO");
                            headline_timer.remove_css_class("clickforge-on");
                            headline_timer.add_css_class("clickforge-off");
                        }
                        details_timer.set_label(&format!("2x {}: {} · 3x {}: {}", if double_enabled { "ON" } else { "OFF" }, parts[2], if triple_enabled { "ON" } else { "OFF" }, parts[3]));
                        window_timer.present();
                        *last_shown_timer.borrow_mut() = Some(Instant::now());
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => { should_quit = true; break; }
                }
            }

            if should_quit {
                window_timer.close();
                return glib::ControlFlow::Break;
            }
            if let Some(surface) = window_timer.surface() {
                let empty_region = gtk::cairo::Region::create();
                surface.set_input_region(&empty_region);
            }
            let should_hide = last_shown_timer.borrow().is_some_and(|shown| shown.elapsed() >= Duration::from_millis(1700));
            if should_hide {
                window_timer.set_visible(false);
                *last_shown_timer.borrow_mut() = None;
            }
            glib::ControlFlow::Continue
        });
        window.set_visible(false);
    });
    app.run_with_args(&["clickforge-osd"]);
    Ok(())
}

fn show_status_notification(
    double_enabled: bool,
    triple_enabled: bool,
    double_rules: &[ClickRule],
    triple_rules: &[ClickRule],
) -> bool {
    let active = double_enabled || triple_enabled;
    let body = describe_modes(double_enabled, triple_enabled, double_rules, triple_rules);
    let mut command = Command::new("notify-send");
    // Il percorso runuser serve solo quando siamo ancora root (es. sessione senza
    // PKEXEC_UID/SUDO_UID). Dopo il rilascio dei privilegi, XDG_RUNTIME_DIR e
    // DBUS_SESSION_BUS_ADDRESS sono già riallineati alla sessione utente.
    if unsafe { libc::geteuid() } == 0 {
        if let (Ok(username), Ok(uid)) = (std::env::var("SUDO_USER"), std::env::var("SUDO_UID")) {
            let runtime_dir = format!("/run/user/{uid}");
            let bus_path = format!("{runtime_dir}/bus");
            if Path::new(&bus_path).exists() {
                command = Command::new("runuser");
                command
                    .arg("-u").arg(username).arg("--").arg("env")
                    .arg(format!("DBUS_SESSION_BUS_ADDRESS=unix:path={bus_path}"))
                    .arg(format!("XDG_RUNTIME_DIR={runtime_dir}"))
                    .arg("PATH=/usr/bin:/bin")
                    .arg("/usr/bin/notify-send");
            }
        }
    }
    command
        .arg("--app-name=ClickForge")
        .arg("--expire-time=1600")
        .arg("--urgency=low")
        .arg("--icon=input-mouse")
        .arg(if active { "ClickForge ATTIVO" } else { "ClickForge DISATTIVO" })
        .arg(body)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_rules_with_independent_counts() {
        let raw = vec!["left:2".to_string(), "right:4".to_string()];
        let rules = parse_rules(&raw, "2x").expect("le regole dovrebbero essere valide");
        assert_eq!(rules.len(), 2);
        assert_eq!(find_rule_count(&rules, KeyCode::BTN_LEFT), Some(2));
        assert_eq!(find_rule_count(&rules, KeyCode::BTN_RIGHT), Some(4));
    }

    #[test]
    fn rejects_duplicate_button_inside_same_profile() {
        let raw = vec!["left:2".to_string(), "left:5".to_string()];
        assert!(parse_rules(&raw, "2x").is_err());
    }

    #[test]
    fn rejects_using_toggle_as_multiplier() {
        let rules = vec![ClickRule { button: MouseButton::Back, clicks: 2 }];
        assert!(validate_rules(&rules, &[], ToggleButton::Back, ToggleButton::Forward).is_err());
    }

    #[test]
    fn pid_file_is_removed_when_guard_drops() {
        let path = std::env::temp_dir()
            .join(format!("clickforge-test-{}-{}.pid", std::process::id(), std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()));
        {
            let guard = write_pid_file(&path).expect("il pid file dovrebbe essere creato");
            assert!(path.exists());
            let contents = std::fs::read_to_string(&path).expect("il pid file dovrebbe essere leggibile");
            assert_eq!(contents.trim(), std::process::id().to_string());
            drop(guard);
        }
        assert!(!path.exists(), "il pid file dovrebbe essere rimosso alla chiusura");
    }

    #[test]
    fn select_requires_both_profiles_enabled_for_double_fallback() {
        let double_rules = vec![ClickRule { button: MouseButton::Middle, clicks: 2 }];
        assert_eq!(select_click_count(KeyCode::BTN_MIDDLE, &double_rules, &[], false, true), None);
        assert_eq!(select_click_count(KeyCode::BTN_MIDDLE, &double_rules, &[], true, false), Some(2));
    }

    #[test]
    fn triple_wins_only_for_buttons_shared_by_both_profiles() {
        let double_rules = vec![
            ClickRule { button: MouseButton::Left, clicks: 2 },
            ClickRule { button: MouseButton::Right, clicks: 4 },
        ];
        let triple_rules = vec![ClickRule { button: MouseButton::Left, clicks: 3 }];
        assert_eq!(select_click_count(KeyCode::BTN_LEFT, &double_rules, &triple_rules, true, true), Some(3));
        assert_eq!(select_click_count(KeyCode::BTN_RIGHT, &double_rules, &triple_rules, true, true), Some(4));
        assert_eq!(select_click_count(KeyCode::BTN_RIGHT, &double_rules, &triple_rules, false, true), None);
        assert_eq!(select_click_count(KeyCode::BTN_LEFT, &double_rules, &triple_rules, true, false), Some(2));
    }
}
