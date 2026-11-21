# Changelog

## 0.7.0

- **Corretto lo scatto del puntatore durante i click moltiplicati:** i click aggiuntivi sono emessi da un thread dedicato; il loop di lettura del mouse fisico non esegue più sleep da `gap_ms`/`press_ms` e continua a inoltrare movimento e rotellina in tempo reale.
- **Loop basato su `poll(2)`** con timeout di 2 ms invece dello sleep fisso di 3 ms.
- **"Ferma" dalla GUI funziona davvero:** il motore rilascia i privilegi di root dopo l'apertura dei dispositivi (torna all'utente di `pkexec`/`sudo` via `PKEXEC_UID`/`SUDO_UID`, con `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`, `WAYLAND_DISPLAY`, `HOME` riallineati), scrive un pid file (`~/.config/clickforge/clickforge.pid`) e la GUI gli invia `SIGTERM`; il motore gestisce `SIGTERM`/`SIGHUP` oltre a `Ctrl+C` e rilascia il mouse fisico. Fallback alla modalità 0.6.0 se il pid file manca.
- Nuova opzione `--pidfile <PATH>`.
- Messaggio chiaro se il dispositivo di input viene scollegato (POLLERR/POLLHUP/POLLNVAL).
- Overlay e notifiche: dopo il rilascio dei privilegi girano direttamente come utente; il percorso `runuser` resta solo per root senza sessione individuabile.
- Nuovi test unitari (pid file, selezione profilo, validazione GUI).
- Workflow CI incluso come `packaging/ci.yml` (architettura: check, test, clippy, build di rilascio su Arch Linux); per attivarlo vedere "CI e packaging" nel README (l'automazione non aveva il permesso `workflows` per `.github/workflows/`). Packaging Arch di esempio in `packaging/PKGBUILD`.
- `Cargo.lock` rimosso da .gitignore (da committare per build riproducibili).

## 0.6.0

- Aggiunta la possibilità di associare più pulsanti a ciascun profilo.
- Conteggio indipendente per ogni pulsante, per ciascun profilo (1–20 click).
- GUI GTK4 aggiornata con selezioni multiple e controlli numerici individuali.
- Interruttori dei profili 2x e 3x configurabili separatamente.
- Priorità 3x limitata ai pulsanti che compaiono in entrambi i profili; gli altri mantengono la regola del proprio profilo.
- Validazione di doppioni e conflitti tra pulsanti moltiplicati e interruttori.
- Migrazione automatica della configurazione 0.5.x al nuovo schema.
- OSD aggiornato per visualizzare tutti i pulsanti e i conteggi di entrambi i profili.
- Aggiunti test unitari per parsing delle regole, conflitti e selezione del profilo.
