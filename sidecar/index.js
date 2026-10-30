'use strict'

// CellKeeper sidecar: tynd mineflayer-wrapper styret af Rust-appen.
// Protokol: én JSON-besked pr. linje. Kommandoer ind på stdin, events ud på stdout.
// Én proces pr. account. Processen lukker sig selv, når stdin lukker.

const readline = require('readline')

// Kun JSON-linjer må skrives til stdout. Alt andet (fx biblioteker der
// bruger console.log) sendes til stderr, så protokollen ikke ødelægges.
const writeOut = process.stdout.write.bind(process.stdout)
const toStderr = (...args) => process.stderr.write(args.map(String).join(' ') + '\n')
console.log = toStderr
console.info = toStderr
console.warn = toStderr
console.debug = toStderr

const mineflayer = require('mineflayer')
const { Authflow, Titles } = require('prismarine-auth')

function send (msg) {
  writeOut(JSON.stringify(msg) + '\n')
}

function log (message) {
  send({ type: 'log', message: String(message) })
}

function stripFormatting (text) {
  return String(text ?? '').replace(/§[0-9a-fk-or]/gi, '').trim()
}

function sleep (ms) {
  return new Promise(resolve => setTimeout(resolve, Math.max(0, Number(ms) || 0)))
}

function clamp (value, fallback, min, max) {
  const n = Number(value)
  if (!Number.isFinite(n)) return fallback
  return Math.max(min, Math.min(max, n))
}

function jitter (base, spread) {
  const s = Math.max(0, Number(spread) || 0)
  return Math.max(0, base + Math.round((Math.random() * 2 - 1) * s))
}

function formatReason (reason) {
  if (!reason) return 'ingen årsag'
  if (typeof reason === 'string') return stripFormatting(reason)
  try {
    return stripFormatting(JSON.stringify(reason))
  } catch {
    return String(reason)
  }
}

// ── Token-cache i hukommelsen ────────────────────────────────────────────
// prismarine-auth accepterer en cache-fabrik i stedet for en mappe.
// Tokens skrives derfor aldrig til disk her; Rust gemmer dem krypteret.
let tokenCache = {}

class MemoryCache {
  constructor (name) {
    this.name = name
  }

  async getCached () {
    return tokenCache[this.name] || {}
  }

  async setCached (value) {
    tokenCache[this.name] = value
    send({ type: 'token_cache', name: this.name, data: value })
  }

  async setCachedPartial (value) {
    await this.setCached({ ...(tokenCache[this.name] || {}), ...value })
  }

  async reset () {
    await this.setCached({})
    return {}
  }
}

const cacheFactory = ({ cacheName }) => new MemoryCache(cacheName)

function onMsaCode (resp) {
  send({
    type: 'msa_code',
    code: resp.user_code,
    url: resp.verification_uri,
    expiresIn: resp.expires_in
  })
}

// ── Bot-tilstand ─────────────────────────────────────────────────────────
let bot = null
let joined = false
let recentChat = [] // { time, text }

function status (state, extra = {}) {
  send({ type: 'status', state, ...extra })
}

function requireBot () {
  if (!bot || !joined) throw new Error('Botten er ikke logget ind på serveren')
  return bot
}

function onChatLine (text) {
  recentChat.push({ time: Date.now(), text })
  if (recentChat.length > 200) recentChat = recentChat.slice(-200)
  send({ type: 'chat', text })
}

async function cmdAuth () {
  const flow = new Authflow('cellkeeper', cacheFactory, {
    authTitle: Titles.MinecraftNintendoSwitch,
    deviceType: 'Nintendo',
    flow: 'live'
  }, onMsaCode)
  const result = await flow.getMinecraftJavaToken({ fetchProfile: true })
  if (!result.profile || !result.profile.name) {
    throw new Error('Microsoft-kontoen ejer ikke Minecraft Java')
  }
  return { name: result.profile.name, uuid: result.profile.id }
}

function cmdConnect (args) {
  if (bot) throw new Error('Allerede forbundet')
  const joinTimeoutMs = clamp(args.joinTimeoutMs, 30000, 1000, 120000)

  return new Promise((resolve, reject) => {
    let done = false
    let timer = null

    const armTimer = () => {
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => finish(new Error(`Timeout: ikke på serveren efter ${joinTimeoutMs / 1000} sek`)), joinTimeoutMs)
    }

    const finish = (err, value) => {
      if (done) return
      done = true
      if (timer) clearTimeout(timer)
      if (err) reject(err)
      else resolve(value)
    }

    status('connecting')
    bot = mineflayer.createBot({
      host: args.host,
      port: Number(args.port) || 25565,
      version: args.version || '1.8.9',
      auth: 'microsoft',
      username: 'cellkeeper',
      profilesFolder: cacheFactory,
      onMsaCode: (resp) => {
        // Brugeren skal logge ind igen: stop join-timeren, mens der ventes.
        if (timer) clearTimeout(timer)
        onMsaCode(resp)
      },
      hideErrors: true,
      // 1.8.9 har ingen chat-signering: spar opkaldet efter certifikater.
      disableChatSigning: true,
      physicsEnabled: true
    })

    armTimer()

    bot.once('login', () => {
      armTimer()
      status('logged_in', { username: bot.username })
    })

    bot.once('spawn', () => {
      joined = true
      status('spawned', { username: bot.username })
      finish(null, { username: bot.username })
    })

    bot.on('messagestr', (message, position) => {
      if (position === 'game_info') return
      const text = stripFormatting(message)
      if (text) onChatLine(text)
    })

    bot.on('kicked', (reason) => {
      status('kicked', { reason: formatReason(reason) })
      finish(new Error(`Smidt ud: ${formatReason(reason)}`))
    })

    bot.on('error', (err) => {
      send({ type: 'error', message: err.message })
      finish(err)
    })

    bot.on('end', (reason) => {
      joined = false
      bot = null
      status('ended', { reason: formatReason(reason) })
      finish(new Error(`Forbindelsen lukkede: ${formatReason(reason)}`))
    })
  })
}

async function cmdAction (args) {
  const b = requireBot()
  const action = String(args.action || '').toLowerCase()
  const moves = ['forward', 'back', 'left', 'right', 'jump', 'sneak', 'sprint']

  if (moves.includes(action)) {
    b.setControlState(action, true)
    await sleep(clamp(args.ms, 350, 0, 30000))
    b.setControlState(action, false)
    return {}
  }
  if (action === 'stopmove') {
    for (const key of moves) b.setControlState(key, false)
    return {}
  }
  if (action === 'hotbar') {
    b.setQuickBarSlot(clamp(args.slot, 1, 1, 9) - 1)
    return {}
  }
  if (action === 'swing') {
    b.swingArm('right')
    return {}
  }
  if (action === 'use') {
    b.activateItem()
    return {}
  }
  if (action === 'deactivate') {
    b.deactivateItem()
    return {}
  }
  if (action === 'drop') {
    if (b.heldItem) await b.tossStack(b.heldItem)
    return {}
  }
  if (action === 'click') {
    const slot = Number(args.slot)
    if (!Number.isInteger(slot)) throw new Error('click kræver et slot-nummer')
    // Vent ikke på serverens kvittering: FreakyVilles menu sender en videre
    // til kø/fængsel uden at svare, og så ville klikket fejle efter 5 sek.
    b.clickWindow(slot, Number(args.button) || 0, Number(args.mode) || 0).catch(() => {})
    return {}
  }
  throw new Error(`Ukendt handling: ${action}`)
}

function cmdChat (args) {
  const b = requireBot()
  const text = String(args.text || '').trim()
  if (!text) throw new Error('Tom chatbesked')
  b.chat(text)
  return {}
}

function isSign (block) {
  return Boolean(block && String(block.name || '').includes('sign'))
}

function signText (block) {
  try {
    const t = block.signText
    if (Array.isArray(t)) return stripFormatting(t.join(' | '))
    if (typeof t === 'string') return stripFormatting(t.replace(/\n/g, ' | '))
  } catch {}
  return ''
}

// Finder nærmeste skilt ud fra afstand (ikke synlighed) og højreklikker.
// activateBlock sender i 1.8 pakken "block_place" (Player Block Placement).
// Der er intet line-of-sight-tjek, så skilte bag en væg virker, så længe
// serveren accepterer afstanden (vanilla 1.8: under 8 blokke).
// JS kender ikke Rusts inline-flag "(?i)". Fjern det, og fald tilbage til
// bogstavelig tekst, hvis mønsteret ikke er gyldigt i JS.
function mkRegex (src) {
  if (!src) return null
  const clean = String(src).replace(/^\(\?i\)/, '')
  try { return new RegExp(clean, 'i') } catch {
    return new RegExp(clean.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'i')
  }
}

async function cmdSign (args) {
  const b = requireBot()
  const clicks = Math.floor(clamp(args.clicks, 9, 0, 1000))
  const delayMs = clamp(args.delayMs, 300, 0, 60000)
  const jitterMs = clamp(args.jitterMs, 0, 0, 5000)
  const maxDistance = clamp(args.maxDistance, 8, 1, 128)

  const sign = b.findBlock({ maxDistance, matching: isSign })
  if (!sign) throw new Error(`Intet skilt inden for ${maxDistance} blokke`)

  const eye = b.entity.position
  const distance = Number(eye.distanceTo(sign.position.offset(0.5, 0.5, 0.5)).toFixed(1))
  const pos = { x: sign.position.x, y: sign.position.y, z: sign.position.z }
  const startedAt = Date.now()

  // Stop så snart serveren siger, at cellen er fuld (fx "Du kan max tilføje 10 dage").
  const stopOn = mkRegex(args.stopOn)
  const sawMax = () => Boolean(stopOn) && recentChat.some(c => c.time >= startedAt && stopOn.test(c.text))
  let done = 0
  let maxReached = false

  for (let i = 0; i < clicks; i++) {
    await b.lookAt(sign.position.offset(0.5, 0.5, 0.5), true)
    await b.activateBlock(sign)
    done = i + 1
    send({ type: 'progress', step: 'sign', done, total: clicks })
    await sleep(jitter(delayMs, jitterMs))
    if (sawMax()) { maxReached = true; break }
  }

  // Giv serveren et øjeblik til at svare på det sidste klik.
  if (!maxReached) {
    await sleep(800)
    maxReached = sawMax()
  }
  const chat = recentChat.filter(c => c.time >= startedAt).map(c => c.text)
  return { clicks: done, maxReached, distance, position: pos, text: signText(sign), chat }
}

// Sender en kommando og samler chatlinjer fra den første linje, der matcher
// "start" (regex), indtil "until" matcher eller der har været stille i settleMs.
// Andre beskeder før start-linjen ignoreres.
function cmdCheckChat (args) {
  const b = requireBot()
  const command = String(args.command || '').trim()
  if (!command) throw new Error('checkchat kræver en kommando')
  const timeoutMs = clamp(args.timeoutMs, 5000, 500, 120000)
  const settleMs = clamp(args.settleMs, 800, 100, 10000)
  const start = mkRegex(args.start)
  const until = mkRegex(args.until)

  return new Promise((resolve, reject) => {
    const lines = []
    let started = false
    let settleTimer = null

    const cleanup = () => {
      clearTimeout(timeoutTimer)
      if (settleTimer) clearTimeout(settleTimer)
      b.removeListener('messagestr', onMessage)
      b.removeListener('end', onEnd)
    }
    const finish = (err) => {
      cleanup()
      if (err) reject(err)
      else resolve({ lines })
    }
    const armSettle = () => {
      if (settleTimer) clearTimeout(settleTimer)
      settleTimer = setTimeout(() => finish(), settleMs)
    }

    function onMessage (message, position) {
      if (position === 'game_info') return
      const text = stripFormatting(message)
      if (!text) return
      if (!started) {
        if (start && !start.test(text)) return
        started = true
      }
      lines.push(text)
      if (until && until.test(text)) return finish()
      armSettle()
    }

    function onEnd (reason) {
      finish(new Error(`Forbindelsen lukkede under chat-tjek: ${formatReason(reason)}`))
    }

    const timeoutTimer = setTimeout(() => {
      if (started) finish()
      else finish(new Error(`Intet svar på "${command}" inden for ${timeoutMs / 1000} sek`))
    }, timeoutMs)

    b.on('messagestr', onMessage)
    b.once('end', onEnd)
    b.chat(command)
  })
}

function cmdLogout () {
  if (!bot) return Promise.resolve({})
  return new Promise((resolve) => {
    const current = bot
    const timer = setTimeout(() => resolve({}), 3000)
    current.once('end', () => {
      clearTimeout(timer)
      resolve({})
    })
    try {
      current.quit('CellKeeper færdig')
    } catch {
      clearTimeout(timer)
      resolve({})
    }
  })
}

const handlers = {
  init: (args) => {
    tokenCache = args.cache && typeof args.cache === 'object' ? args.cache : {}
    return {}
  },
  auth: cmdAuth,
  connect: cmdConnect,
  action: cmdAction,
  chat: cmdChat,
  sign: cmdSign,
  checkchat: cmdCheckChat,
  logout: cmdLogout
}

async function handle (line) {
  let msg
  try {
    msg = JSON.parse(line)
  } catch {
    send({ type: 'error', message: 'Ugyldig JSON fra appen' })
    return
  }
  const { id, cmd } = msg
  const handler = handlers[cmd]
  if (!handler) {
    send({ type: 'result', id, ok: false, error: `Ukendt kommando: ${cmd}` })
    return
  }
  try {
    const data = await handler(msg)
    send({ type: 'result', id, ok: true, data: data || {} })
  } catch (err) {
    send({ type: 'result', id, ok: false, error: err && err.message ? err.message : String(err) })
  }
}

let shuttingDown = false
async function shutdown (code = 0) {
  if (shuttingDown) return
  shuttingDown = true
  try {
    await cmdLogout()
  } finally {
    process.exit(code)
  }
}

process.on('uncaughtException', (err) => {
  send({ type: 'error', message: `Uventet fejl: ${err.message}` })
  shutdown(1)
})
process.on('unhandledRejection', (err) => {
  send({ type: 'error', message: `Uventet fejl: ${err && err.message ? err.message : err}` })
})

const rl = readline.createInterface({ input: process.stdin })
rl.on('line', (line) => {
  if (line.trim()) handle(line)
})
// Appen er lukket eller crashet: log ud og afslut.
rl.on('close', () => shutdown(0))

send({ type: 'ready', version: 1 })
