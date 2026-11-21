# Changelog

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
