# CellKeeper

Forny celledage på FreakyVille med ét klik – for alle dine Minecraft-accounts.

CellKeeper logger hver account ind i baggrunden, klikker på cellens skilt til
cellen er fuld (9 dage), tjekker resultatet med `/ce info`, logger ud igen og
opdaterer cell-manager automatisk.

> **Brug på eget ansvar.** Automatisering kan være i strid med FreakyVilles
> regler og kan i værste fald give ban. Appen holder pauser og tilfældige
> forsinkelser mellem logins og klik, men kan ikke garantere, at serveren
> ikke opdager den.

---

## Kom i gang

1. Hent installeren (`CellKeeper_x.y.z_x64-setup.exe`) og kør den.
   Windows kan vise "Windows beskyttede din pc", fordi appen ikke er
   kodesigneret. Tryk **Flere oplysninger → Kør alligevel**.
2. Åbn **Accounts → + Tilføj account → Start login**.
3. Appen viser en kode. Tryk **Åbn microsoft.com/link**, skriv koden og log
   ind med den Microsoft-konto, der ejer Minecraft. Gentag for hver account.
4. Gå til **Celler**. Cellerne hentes automatisk fra cell-manager og kobles
   til den account, der har samme Minecraft-navn.
5. Tryk **Tilføj dage**.

> **Husk:** Accounten skal være **logget ud af Minecraft** (du må ikke spille
> på den samtidig), og den skal være logget ud stående **inde i sin celle tæt
> på skiltet**. Det er dér, den dukker op, når appen logger ind.

## Sådan virker "Tilføj dage"

For hver valgt celle:

1. Accounten logger ind på `mc.freakyville.dk` (Minecraft 1.8.9).
2. Join-makroen kører (standard: åbn server-menuen og klik på slot 20).
3. `/ce info <celle>` læser, hvor mange dage cellen har nu.
4. Appen klikker på det nærmeste skilt det antal gange, der mangler
   (1 klik = 1 dag). Ingen klik for meget.
5. `/ce info <celle>` igen. Er cellen ikke fuld, klikker den forskellen
   igen (højst 3 runder).
6. Accounten logger ud – også hvis noget gik galt.
7. cell-manager opdateres med det antal dage, `/ce info` viste.

Resultatet vises live under **Kørsel**:

- ✓ **grøn**: verificeret fuld, cell-manager opdateret.
- **!** **gul**: købet gik igennem, men cellen er ikke fuld. Du ser det
  faktiske antal og kan selv vælge at sende det til cell-manager.
- ✕ **rød**: fejl (fx intet skilt, login udløbet). Tryk **Prøv igen** for
  kun den celle.

Lukker appen midt i en kørsel, viser den en knap **Genoptag** næste gang.

## Skilte gennem vægge

Skiltet findes ud fra **afstand, ikke synlighed**. Det må gerne være bag en
væg. Teknisk sender appen samme pakke som et højreklik
(`block_place` / Player Block Placement i 1.8).

- Serveren afviser normalt klik længere væk end ca. 8 blokke, og den giver
  ingen besked om det. Derfor er standard-afstanden 8.
- Kun `/ce info` beviser, at købet lykkedes. Appen stoler aldrig på klikket
  alene.

## Flyt til en anden PC

**Indstillinger → Flyt til anden PC**:

1. **Eksportér accounts** på den gamle PC. Vælg et kodeord (mindst 8 tegn).
   Du får én krypteret `.cellkeeper`-fil.
2. Flyt filen (USB, mail til dig selv osv.).
3. **Importér accounts** på den nye PC og skriv kodeordet.

Logins følger med, så du slipper for at logge ind igen. Hvis Microsoft
alligevel afviser et login (fx efter ca. 90 dage uden brug), beder appen om
en ny kode – det tager et halvt minut.

Du kan også importere `accounts.json` fra headless-client. Så kommer navne
og join-makroer med, men hver account skal logge ind én gang.

## cell-manager

- Adresse og API-nøgle sættes under **Indstillinger → cell-manager**.
  **Test forbindelse** tjekker begge dele.
- cell-manager bestemmer cellenavn, account, rank, block og Discord-bruger.
  CellKeeper ændrer aldrig de felter.
- CellKeeper skriver kun antal dage – og kun et tal, den lige har læst med
  `/ce info`.
- Celler, der kun findes i appen ("lokal"), sendes aldrig til cell-manager.

## Makroer

Under **Indstillinger → Makroer** (og pr. account under **Redigér**).
Gamle makroer fra headless-client virker uændret.

Standard-fornyelsesmakro:

```text
start
waitjoin 30000
joinmacro
fyldop
```

Standard-join-makro:

```text
use
wait 1000
click 20
wait 5000
```

Nye kommandoer:

- `fyldop [runder]` – læs `/ce info`, klik det der mangler, tjek igen.
- `ceinfo [mål]` – læs `/ce info` (standard: cellen).

Variabler: `{celle}`, `{konto}`, `{max}`, `{antal}`
(`{antal}` = dage der mangler ifølge seneste `/ce info`).

## Sikkerhed

- Microsoft-logins gemmes krypteret (XChaCha20-Poly1305). Nøglen ligger i
  Windows Legitimationsoplysninger. Appen ser aldrig din adgangskode.
- Eksport-filer er krypteret med dit kodeord (Argon2id).
- Intet uploades. Den eneste netværkstrafik er til Minecraft-serveren,
  Microsofts login og din cell-manager.
- Hver account kører i sin egen skjulte proces, som altid lukkes, når appen
  lukker (også ved nedbrud).

## Fejlsøgning

- **"Intet skilt inden for 8 blokke"**: accounten står ikke ved sin celle.
  Log ind manuelt, gå ind i cellen, og log ud igen.
- **"Klikkene gav ingen ekstra dage"**: skiltet er for langt væk, købet blev
  afvist, eller der mangler penge.
- **"Fandt ikke /ce info-overskriften"**: serveren svarede ikke, eller
  formatet har ændret sig. Se loggen (knappen **Log**).
- **Login udløbet**: appen viser en ny kode midt i kørslen. Log ind, så
  fortsætter den.

---

## Udvikling

Krav: Node 22, Rust (stable) og Tauri's systemkrav.

```bash
cd sidecar && npm ci && node build.mjs   # bygger sidecar til src-tauri/binaries
cd .. && npx @tauri-apps/cli@2 dev       # starter appen
npx @tauri-apps/cli@2 build              # installer i src-tauri/target/release/bundle
```

GitHub Actions bygger Windows-installeren ved hvert push
(`.github/workflows/build.yml`). Den ligger som artefakt på kørslen.

Struktur:

- `src-tauri/` – Rust: kørsel, makro-sprog, `/ce info`-parser, kryptering,
  cell-manager-klient.
- `sidecar/` – lille Node-program med mineflayer (login, skilt, chat).
  Pakkes som én exe med Node SEA. minecraft-data beskæres til 1.8.
- `ui/` – HTML/CSS/JS uden framework.
- `docs/PLAN.md` – research og arkitektur.
