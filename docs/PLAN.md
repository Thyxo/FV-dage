# CellKeeper – research og plan

Status: **Fase 0 + Fase 1 færdig, åbne spørgsmål besvaret.
Venter på "ja" til Fase 2.**

---

## Fase 0 – Fund

**headless-client** (`index.js`, 1535 linjer)

1. Én fil: `http`-server + indlejret HTML + mineflayer. Makro-sættet og
   `runSignWorkflow` er præcis som beskrevet i opgaven (verificeret).
2. `sign` = `findBlock` (navn indeholder "sign") → `lookAt` → `activateBlock`.
   I 1.8 sender det pakken **`block_place`** (Player Block Placement, 0x08)
   med `direction = 1` (oversiden) og cursor 8/8/8. Ingen line-of-sight-tjek.
3. `checkchat` tager første ikke-tomme besked. `validatechat` tester kun den.
4. Tokens: mineflayer → minecraft-protocol → prismarine-auth. Cache ligger i
   `%APPDATA%\.minecraft\nmp-cache\<sha1(mail)[0..6]>_{live,xbl,mca}-cache.json`
   som **ukrypteret JSON**. Flow = `live` (Nintendo Switch-klient-id).
5. `accounts.json` i repoet er ét objekt (ikke en liste), har BOM og
   indeholder **rigtig mail** (committet 2026-06-26). `loadAccounts()` kan
   faktisk ikke parse filen og overskriver den med default-bot.
6. Den gemte makro har join-trin inline (`use`, `wait 1000`, `click 20`,
   `wait 5000`) i stedet for `joinmacro`.

**cell-manager**

7. API'et virker live: `backend-production-a068.up.railway.app` (hardcodet i
   `bot/src/lib/backend.js`). 24 celler, 22 accounts, maks 2 celler/account.
8. **Alle 24 celler står på 0 dage** lige nu. Botten sender derfor DM'er hver
   12. time for alle 24 – uden ende.
9. Cellenavne er korte (`a1615`, `c357`, `fc77`), ingen mellemrum.
   `accountName` = Minecraft-navn (blandet store/små bogstaver).
10. Ingen auth på API'et. Repo + URL er offentlige → alle kan ændre/slette.
11. Midnats-nedtælling kører i serverens tidszone = UTC på Railway
    (kl. 01/02 dansk tid).

## Afgjort under research

**Kan tokens flyttes til anden PC?** Ja. `live`-flowets refresh-token er
ikke bundet til maskinen. Bonus: prismarine-auth accepterer en
*cache-funktion* i stedet for en mappe. Sidecaren holder derfor tokens i
hukommelsen og sender dem til Rust, som gemmer dem krypteret.
**Ingen token-filer i klartekst på disken.**
Forbehold: refresh-token udløber efter ca. 90 dage uden brug, og Microsoft
kan afvise dem. Så tilbyder appen et hurtigt re-login (kode + link, ~30 sek).

**Pakning af sidecar** (testet her på Linux, Windows-exe krydsbygget):

- Uden beskæring: **411 MB** (minecraft-data har alle versioner med).
- Beskåret til kun 1.8: bundle **2,2 MB**.
- `bun build --compile`: virker, 117 MB Windows-exe, men bun mangler
  native AES-CFB8 → falder tilbage til ren JS (1,5 MB/s) → langsomme joins.
- **Node SEA (Node 22): virker, native kryptering. Valgt.**
  Forventet ca. 85 MB på Windows, ca. 30-40 MB i komprimeret installer.

## Afklaret med brugeren (2026-10-27)

1. `/ce info <account>` og `/ce info <celle>` giver samme svar.
   → Appen bruger cellenavnet (unikt), med account-navn som reserve.
2. 9 klik = 9 dage fra 0. Der kommer (sandsynligvis) en chatbesked pr. klik.
   Alle dage skal med. → "Fyld op"-flow, se nedenfor.
3. Accounts står **inde i cellen, tæt på skiltet**, når de logger ud.
   Skiltet er altså typisk bag en væg → "gennem væggen" er kernen.
   Noten rettes til: *"Accounten skal være logget ud af Minecraft og skal
   være logget ud stående inde i sin celle tæt på skiltet."*
4. Ejer-accounten klikker selv. → Ét flow pr. account.
5. Ja til API-nøgle. Celler i appen og i cell-manager skal være **linket**,
   så de ikke overskriver hinanden. → Se "Kobling til cell-manager".

## Stadig åbent (blokerer ikke Fase 2)

- Et rigtigt `/ce info`-svar (hele chatlinjen). Indtil da: parseren tager
  tallet foran "dag/dage" og viser rå tekst, så du kan bekræfte første gang.
- Accounts med 2 celler (fx CokeBums: `a1615` + `fc77`): kan begge skilte
  nås fra samme sted? Ellers kan appen kun forny den ene automatisk.

---

## Fase 1 – Plan

### UI: Tauri v2 (ikke egui)

- Sidecar via `externalBin` er indbygget.
- HTML/CSS gør det let at lave pænt, mørkt og responsivt til små skærme.
- WebView2 findes allerede på Windows 10/11. App-exe ca. 8-12 MB.
- Frontend: ren HTML/CSS/JS uden framework og uden bundler.

### Arkitektur

```
┌──────────── CellKeeper.exe (Rust/Tauri) ────────────┐
│ UI (webview)  ⇄  Tauri-kommandoer + events           │
│                                                      │
│ Orchestrator (tokio)                                 │
│  ├─ run-journal (genoptag efter crash)               │
│  ├─ kø: maks N samtidige bots + jitter               │
│  ├─ verifikation (/ce info-parser)                   │
│  └─ cell-manager-klient (reqwest)                    │
│ Lager                                                │
│  ├─ state.json   (accounts, celler, makroer)         │
│  └─ vault.bin    (tokens, XChaCha20-Poly1305)        │
│      nøgle i Windows Credential Manager              │
└───────┬───────────────────────────┬──────────────────┘
        │ stdin/stdout JSON-linjer  │ HTTPS
  ┌─────▼──────┐  én pr. account   ┌▼──────────────┐
  │ sidecar    │ ─────────────────►│ cell-manager  │
  │ (Node SEA, │  mc.freakyville   │ PATCH /days   │
  │ mineflayer)│  .dk:25565, 1.8.9 └───────────────┘
  └────────────┘
```

### Sidecar-protokol (JSON-linjer)

Ind (fra Rust):

- `login {account, cache}` – cache = krypteret-lager-indhold, dekrypteret
- `auth {username}` – kun Microsoft-login (device code), ingen server-join
- `macro {script, vars}` – `{celle}`, `{antal}` udskiftes i Rust
- `sign {clicks, delayMs, maxDistance}`
- `checkchat {command, match, timeoutMs}` – venter på *matchende* besked
- `logout`

Ud (til Rust):

- `status` (connecting / online / spawned / stopped)
- `chat`, `log`, `error`
- `msa_code {code, url, expiresIn}` – vises i appen
- `token_cache {name, data}` – logges aldrig
- `result {id, ok, data}`

Sidecaren afslutter selv, når stdin lukker. Rust lægger alle sidecars i et
Windows Job Object med "kill on close" → dør også hvis appen crasher.

### Makro-motor

- Parser i **Rust** (testbar). Kommandoer 1:1 med headless-client, så gamle
  makroer virker. Sidecaren får ét trin ad gangen.
- `checkchat` forbedret: venter på besked der matcher `{celle}` eller regex,
  ignorerer andet, timeout + retry (standard 2 forsøg).
- `joinmacro` = redigerbar tekstboks pr. account, med global default.

### "Tilføj dage" – trin pr. account ("fyld op")

```
1 Start sidecar   →  2 login + waitjoin
3 joinmacro
4 /ce info <celle>   → nu = fx 3 dage
5 klik (9 − nu) gange, tæl kvitteringer i chat
6 /ce info <celle>   → er den 9?
   nej → klik forskellen igen (maks 2 runder)
   stadig nej → rød advarsel med faktisk antal
7 logout (altid, også ved fejl)
8 cell-manager opdateres (kun verificeret værdi)
```

Fordele: ingen klik for meget (spild af penge), og appen stopper først,
når `/ce info` bekræfter 9 dage. Maks-dage (9) er en indstilling.

Hvert trin skrives til `runs/<id>.json` før og efter. Efter crash viser
appen "Ufærdig kørsel" med knap til at genoptage pr. account eller celle.

### Skilte gennem vægge

- Bevarer `findBlock`-kernen (afstand, ikke synlighed).
- **Fund:** vanilla 1.8 afviser stille interaktioner længere væk end 8 blokke.
  Default `maxDistance` foreslås derfor 8 (i stedet for 16), og appen
  advarer hvis skiltet er >6 blokke væk.
- Klik giver ingen kvittering fra serveren. Kun `/ce info` beviser købet.

### Kobling til cell-manager (så de ikke overskriver hinanden)

Hvert felt ejes af ét system:

- **cell-manager ejer:** cellenavn, account, rank, block, Discord-bruger.
  Appen henter dem og ændrer dem aldrig.
- **Spillet ejer dage:** appen skriver kun `daysLeft`, og kun en værdi den
  lige har læst med `/ce info`. Aldrig en gammel lokal værdi.
- **Appen ejer lokalt:** makroer, skilt-indstillinger, tokens.

Linket gemmes som cell-managers `_id` (overlever omdøbning) + cellenavn.
Celler oprettes i cell-manager (dashboard/Discord), appen importerer dem.
Celler der kun findes lokalt vises som "ikke linket" og sendes aldrig.
Visning af dage i appen kommer altid fra cell-manager (live).

### Mappestruktur

```
FV-dage/
├─ src-tauri/            Rust
│  ├─ src/
│  │  ├─ main.rs
│  │  ├─ macro_lang.rs   parser + tests
│  │  ├─ ceinfo.rs       /ce info-parser + tests
│  │  ├─ verify.rs       verifikation + tests
│  │  ├─ orchestrator.rs kø, retry, journal
│  │  ├─ sidecar.rs      proces + JSON-linjer
│  │  ├─ vault.rs        kryptering, export/import
│  │  └─ cellmanager.rs  REST-klient
│  └─ tauri.conf.json
├─ ui/                   index.html, app.css, app.js
├─ sidecar/              Node: index.js, build.mjs
├─ docs/PLAN.md
└─ README.md             dansk brugervejledning
```

### Crates

`tauri 2`, `tokio`, `serde`/`serde_json`, `reqwest` (rustls), `keyring`,
`chacha20poly1305`, `argon2`, `rand`, `regex`, `thiserror`, `tracing`.

### Kryptering

- Lokalt: tilfældig 256-bit nøgle i Windows Credential Manager (`keyring`),
  `vault.bin` krypteret med XChaCha20-Poly1305.
- Eksport: Argon2id (64 MiB, 3 iterationer) + XChaCha20-Poly1305,
  kodeord vælges af brugeren. Én `.cellkeeper`-fil. Aldrig cloud.
- Import accepterer også headless-clients `accounts.json` (objekt eller liste,
  BOM fjernes) – kun navne/mail, tokens kræver login.

### Risici

1. **Exe-størrelse:** ca. 85 MB sidecar. Kræver beskæring af minecraft-data
   i build-scriptet (ellers 400+ MB).
2. **Antivirus/SmartScreen:** usigneret exe der starter Node-processer →
   "Ukendt udgiver". Løsning: kodesignering (ca. 1.500-3.000 kr/år) eller
   "Kør alligevel" i README.
3. **Serverregler:** automatisering kan bryde FreakyVilles regler → ban.
   Jitter, én login ad gangen som standard, README-advarsel.
4. **Rækkevidde/anti-cheat:** klik >8 blokke eller gennem vægge kan blive
   afvist uden besked. Fanges af `/ce info`.
5. **RAM:** ca. 100 MB pr. bot-proces → maks 3 samtidige (indstilling).
6. **Microsoft:** prismarine-auth bruger Nintendo Switch-klient-id. Kan
   blive blokeret en dag → re-login-flow som nødudgang.
7. **Test:** jeg kan ikke teste mod FreakyVille eller med rigtige accounts
   herfra. Plan: lokal offline 1.8.9-testserver (flying-squid) til
   sign/chat-flow + enhedstests. Windows-build via GitHub Actions.

### Tests

- Rust: makro-parser, `/ce info`-parser, verifikation, journal, kryptering
  (round-trip), cell-manager-klient mod mock.
- Sidecar: integrationstest mod lokal 1.8.9-server (offline auth).

---

## Cell-manager – forslag (implementeres ikke uden ok)

1. **API-nøgle** på POST/PUT/PATCH/DELETE (`x-api-key`, env `API_KEY`).
   Vigtigst: URL'en står i et offentligt repo. **Godkendt af brugeren.**
2. **Felter** `lastRenewedAt` + `verifiedAt` (sættes når CellKeeper
   opdaterer). Dashboardet viser "Fornyet for 3 t siden ✓".
3. **Batch** `PATCH /api/cells/days` med liste → én socket-emit i stedet for
   22, og kun én notifikations-runde i botten.
4. **Udløbsdato i stedet for nedtælling:** gem `expiresAt` og beregn dage.
   Så forsvinder midnats-cronen, UTC-forskydningen og drift efter genstart.
5. **Stop DM-spam ved 0 dage:** status "mistet" efter X dage på 0, og
   validér `daysLeft` som heltal i `PATCH /days`.

MongoDB/Railway kan beholdes – det virker, og en omlægning giver ikke nok.
