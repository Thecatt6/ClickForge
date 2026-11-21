# ClickForge 0.6.0

ClickForge è un moltiplicatore di click per Linux scritto in Rust. Legge gli eventi del mouse fisico tramite `evdev`, acquisisce il dispositivo in esclusiva e inoltra gli eventi tramite un mouse virtuale `uinput`. È pensato per funzionare sia con Wayland sia con X11, indipendentemente dall'ambiente desktop.

## Novità della 0.6.0

- **Più pulsanti per profilo:** puoi moltiplicare contemporaneamente sinistro, destro, centrale e pulsanti laterali.
- **Conteggio indipendente per ogni pulsante:** per esempio sinistro 2x e destro 4x nello stesso profilo.
- **Due profili indipendenti:** ognuno ha un interruttore hardware separato, il proprio elenco di pulsanti e i conteggi individuali.
- **Priorità prevedibile per pulsante:** se lo stesso pulsante è presente in entrambi i profili e i due profili sono attivi, per quel pulsante prevale il conteggio 3x. Gli altri pulsanti continuano a usare il proprio profilo.
- **GUI GTK4 opzionale:** configura tutti i pulsanti senza modificare file o ricordare argomenti CLI. La modalità terminale continua a essere disponibile.
- **Migrazione automatica delle impostazioni 0.5.x:** il vecchio `config.toml` viene convertito nel nuovo schema al primo avvio della GUI e può essere salvato nel formato aggiornato.
- **Validazione delle assegnazioni:** impedisce di utilizzare lo stesso pulsante come interruttore e come pulsante moltiplicato, o di assegnare due interruttori identici.
- **Overlay click-through Wayland**, notifica KDE alternativa o nessun OSD.
- Rilevamento automatico del mouse, selezione manuale e log diagnostico.
- Movimento, rotellina e pulsanti non assegnati continuano a essere inoltrati.
- Le pressioni prolungate non vengono moltiplicate, per evitare click aggiuntivi durante il trascinamento.

## Requisiti su CachyOS / Arch Linux

```bash
sudo pacman -S --needed rustup base-devel gtk4 gtk4-layer-shell polkit
rustup default stable
sudo modprobe uinput
```

Per usare le notifiche KDE alternative all'overlay:

```bash
sudo pacman -S --needed libnotify
```

## Compilazione

Dalla cartella che contiene `Cargo.toml`:

```bash
cargo build --release
```

Cargo scarica automaticamente le crate Rust necessarie. L'eseguibile sarà `target/release/clickforge`.

## Avvio rapido da terminale

```bash
sudo target/release/clickforge
```

Il mouse viene rilevato automaticamente. La configurazione predefinita è:

| Pulsante | Profilo | Conteggio | Interruttore |
|---|---|---:|---|
| Sinistro | 2x | 2 click | Laterale posteriore (`BTN_SIDE`) |
| Sinistro | 3x | 3 click | Laterale anteriore (`BTN_EXTRA`) |

Il profilo 2x parte attivo; il 3x parte disattivo.

## GUI opzionale

Avvia la GUI **senza `sudo`**:

```bash
./target/release/clickforge --gui
```

La GUI permette di:

- selezionare più pulsanti per il profilo 2x e più pulsanti per il profilo 3x;
- scegliere separatamente il conteggio (da 1 a 20 click) per ogni pulsante in ogni profilo;
- scegliere quale pulsante fisico attiva/disattiva ciascun profilo;
- scegliere lo stato iniziale dei profili;
- selezionare il mouse automaticamente (`auto`) o indicare `/dev/input/eventX`;
- scegliere overlay, notifica KDE o nessun indicatore;
- regolare intervallo tra i click, durata della pressione virtuale e soglia per i trascinamenti;
- salvare la configurazione, avviare/fermare il motore e aprire il log.

La GUI non viene eseguita come root. Quando premi **Avvia ClickForge**, `pkexec` richiede l'autorizzazione per il solo motore che deve accedere a `/dev/input` e `/dev/uinput`.

La configurazione è salvata in:

```text
~/.config/clickforge/config.toml
```

Il log è salvato in:

```text
~/.config/clickforge/clickforge.log
```

## Configurare più pulsanti da terminale

Gli argomenti `--double-button` e `--triple-button` sono ripetibili. Il formato è `PULSANTE:NUMERO_CLICK`.

Esempio: nel profilo 2x il sinistro produce 2 click e il destro 4; nel profilo 3x il sinistro produce 3 click e il destro 5:

```bash
sudo target/release/clickforge \
  --double-button left:2 \
  --double-button right:4 \
  --triple-button left:3 \
  --triple-button right:5
```

Puoi associare pulsanti diversi ai due profili. Per esempio, il profilo 2x può moltiplicare sinistro e destro, mentre il 3x può moltiplicare solo il pulsante centrale:

```bash
sudo target/release/clickforge \
  --double-button left:2 \
  --double-button right:2 \
  --triple-button middle:3
```

Pulsanti disponibili: `left`, `right`, `middle`, `back`, `forward`.

### Come si comportano i profili simultanei

I profili sono indipendenti. Se sono entrambi attivi:

- un pulsante assegnato solo al profilo 2x usa il conteggio 2x;
- un pulsante assegnato solo al profilo 3x usa il conteggio 3x;
- un pulsante assegnato a entrambi usa il conteggio 3x;
- i conteggi non vengono mai sommati.

Ogni click fisico viene inoltrato una sola volta e ClickForge genera solo i click aggiuntivi necessari.

## Pulsanti di attivazione

Per impostazione predefinita:

- `BTN_SIDE` / `back`: attiva o disattiva il profilo 2x;
- `BTN_EXTRA` / `forward`: attiva o disattiva il profilo 3x.

Puoi cambiare gli interruttori:

```bash
sudo target/release/clickforge \
  --double-toggle-button back \
  --triple-toggle-button forward
```

Valori consentiti: `left`, `right`, `middle`, `back`, `forward`, `none`.

Un pulsante assegnato come interruttore viene consumato e non esegue la propria azione originale nelle applicazioni. Per evitare conflitti, non è consentito usare lo stesso pulsante sia come interruttore sia come pulsante moltiplicato. I due interruttori devono essere diversi; uno può essere impostato su `none`.

## Stato iniziale dei profili

```bash
sudo target/release/clickforge --double-enabled true --triple-enabled false
```

Un profilo impostato su `none` parte sempre disattivato perché non avrebbe un pulsante per riattivarlo.

## Selezione manuale del mouse

```bash
sudo target/release/clickforge --list
sudo target/release/clickforge --device /dev/input/eventX
```

Se `--device` viene omesso o impostato su `auto`, ClickForge cerca un dispositivo che esponga tutti i pulsanti richiesti dai profili e dagli interruttori. Quando sono presenti più candidati, mostra i dispositivi trovati e sceglie quello con il punteggio più alto.

## OSD

```bash
sudo target/release/clickforge --osd overlay       # predefinito, Wayland Layer Shell
sudo target/release/clickforge --osd notification  # notifica KDE
sudo target/release/clickforge --osd none           # nessun indicatore
```

L'OSD indica lo stato dei due profili e l'elenco dei pulsanti/conteggi assegnati.

## Regolazione della temporizzazione

```bash
sudo target/release/clickforge --gap-ms 55 --press-ms 15 --max-hold-ms 250
```

- `--gap-ms`: intervallo tra il click fisico e ogni click aggiuntivo (0-1000 ms).
- `--press-ms`: durata della pressione virtuale (1-100 ms).
- `--max-hold-ms`: pressioni più lunghe vengono trattate come trascinamenti (1-2000 ms).

## Opzioni principali

```text
--gui                           Apre l'interfaccia grafica
--list                          Elenca i dispositivi input
--device <PATH>                 Dispositivo /dev/input/eventX; default: automatico
--double-button <BUTTON:COUNT>  Regola profilo 2x, ripetibile
--triple-button <BUTTON:COUNT>  Regola profilo 3x, ripetibile
--double-toggle-button <BUTTON> Interruttore profilo 2x (default: back)
--triple-toggle-button <BUTTON> Interruttore profilo 3x (default: forward)
--double-enabled <BOOL>         Stato iniziale 2x (default: true)
--triple-enabled <BOOL>         Stato iniziale 3x (default: false)
--osd <MODE>                    overlay, notification, none
--gap-ms <MS>                   Intervallo tra click aggiuntivi (default: 45)
--press-ms <MS>                 Durata click virtuale (default: 12)
--max-hold-ms <MS>              Soglia pressione lunga (default: 250)
-h, --help                      Mostra l'aiuto
-V, --version                   Mostra la versione
```

Il vecchio argomento `--click-button` e i relativi conteggi `--double-clicks` / `--triple-clicks` restano disponibili per compatibilità con i comandi della versione 0.5.x. Per nuove configurazioni si consiglia il formato ripetibile `--double-button` / `--triple-button`.

## Note di funzionamento e sicurezza

- Avviare il motore richiede accesso a `/dev/input` e `/dev/uinput`; da terminale si usa `sudo`, dalla GUI viene richiesto `pkexec`.
- Il mouse fisico viene acquisito in esclusiva; gli eventi passano attraverso il mouse virtuale ClickForge. Se il programma termina inaspettatamente, il kernel rilascia il dispositivo quando il processo chiude il file descriptor.
- I pulsanti configurati come interruttori vengono intercettati e non inoltrati alle applicazioni.
- Le pressioni lunghe oltre `max-hold-ms` non generano click aggiuntivi, così da non moltiplicare i trascinamenti.
- Il profilo selezionato viene fotografato alla pressione del pulsante: cambiare profilo mentre un pulsante è già premuto non cambia il conteggio di quel singolo click.
- Se il mouse espone interfacce evdev separate, può essere necessario specificare manualmente il dispositivo che contiene tutti i pulsanti usati.
- Per uscire dalla modalità terminale, premi `Ctrl+C`.

## Dipendenze Rust

- `evdev`
- `clap`
- `ctrlc`
- `anyhow`
- `libc`
- `serde`
- `toml`
- `gtk4`
- `gtk4-layer-shell`

Licenza: MIT.
