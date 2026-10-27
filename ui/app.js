'use strict'

const { invoke } = window.__TAURI__.core
const { listen } = window.__TAURI__.event

const $ = (s) => document.querySelector(s)
const $$ = (s) => [...document.querySelectorAll(s)]

let view = null
const selected = new Set()
const knownCells = new Set()
let tab = 'cells'
let filter = ''
let loginFor = null
let loginBusy = false
let codeUrl = ''
let editId = null
let transferMode = 'export'
const logLines = []

// ── Hjælpere ──────────────────────────────────────────────────────────────

function esc (v) {
  return String(v ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]))
}

let toastTimer = null
function toast (msg, kind = '') {
  const el = $('#toast')
  el.textContent = msg
  el.className = `toast ${kind}`
  clearTimeout(toastTimer)
  toastTimer = setTimeout(() => el.classList.add('is-hidden'), kind === 'bad' ? 7000 : 4000)
}

async function call (cmd, args = {}) {
  try {
    return await invoke(cmd, args)
  } catch (e) {
    toast(String(e), 'bad')
    throw e
  }
}

function setView (v) {
  if (v) view = v
  render()
}

const head = (name) => `https://mc-heads.net/avatar/${encodeURIComponent(name || 'MHF_Steve')}/32`
const isWide = () => window.matchMedia('(min-width: 980px)').matches

function daysClass (d) {
  if (d == null) return ''
  if (d <= 1) return 'r'
  if (d <= 4) return 'y'
  return 'g'
}

function ago (iso) {
  if (!iso) return ''
  const s = (Date.now() - new Date(iso).getTime()) / 1000
  if (s < 90) return 'lige nu'
  if (s < 3600) return `for ${Math.round(s / 60)} min siden`
  if (s < 172800) return `for ${Math.round(s / 3600)} t siden`
  return `for ${Math.round(s / 86400)} dage siden`
}

function fmtTime (iso) {
  if (!iso) return ''
  return new Date(iso).toLocaleString('da-DK', { day: 'numeric', month: 'numeric', hour: '2-digit', minute: '2-digit' })
}

// ── Valg af celler ────────────────────────────────────────────────────────

const canRun = (c) => !c.problem
const needsDays = (c) => c.daysLeft == null || c.daysLeft < view.settings.maxDays

function syncSelection () {
  const ids = new Set(view.cells.map((c) => c.id))
  for (const c of view.cells) {
    if (!knownCells.has(c.id)) {
      knownCells.add(c.id)
      if (canRun(c) && needsDays(c)) selected.add(c.id)
    }
    if (!canRun(c)) selected.delete(c.id)
  }
  for (const id of [...selected]) if (!ids.has(id)) selected.delete(id)
}

const visibleCells = () => view.cells.filter((c) => !filter || c.accountId === filter)
const chosenIds = () => visibleCells().filter((c) => selected.has(c.id)).map((c) => c.id)

// ── Render ────────────────────────────────────────────────────────────────

function render () {
  if (!view) return
  syncSelection()
  renderTop()
  renderFilter()
  renderCells()
  renderAccounts()
  renderRun()
  renderRunbar()
}

function renderTop () {
  const m = view.manager
  const chip = $('#mgr')
  chip.classList.toggle('ok', m.ok === true)
  chip.classList.toggle('bad', m.ok === false)
  $('#mgr-text').textContent = m.ok === false
    ? `cell-manager: ${m.message}`
    : m.lastSync ? `cell-manager · ${ago(m.lastSync)}` : 'cell-manager'

  $('#notice').classList.toggle('is-hidden', !view.notice)
  $('#notice-text').textContent = view.notice || ''

  const run = view.run
  const missing = run && run.state === 'interrupted' && !view.runActive
    ? run.items.filter((i) => ['pending', 'failed', 'cancelled'].includes(i.state)).length
    : 0
  $('#resume').classList.toggle('is-hidden', !missing)
  $('#resume-text').textContent = `Sidste kørsel blev afbrudt. ${missing} celle(r) mangler.`
}

function renderFilter () {
  const sel = $('#acct-filter')
  const accounts = view.accounts.filter((a) => a.cellCount > 0)
  if (filter && !accounts.some((a) => a.id === filter)) filter = ''
  sel.innerHTML = '<option value="">Alle accounts</option>' +
    accounts.map((a) => `<option value="${esc(a.id)}">${esc(a.label || a.name)}</option>`).join('')
  sel.value = filter
}

function renderRunbar () {
  const n = chosenIds().length
  $('#run-hint').textContent = view.runActive ? 'Kørsel i gang …' : n ? `${n} celle(r) valgt` : 'Vælg celler nedenfor'
  $('#btn-run').disabled = view.runActive || n === 0
  $('#btn-run').classList.toggle('is-hidden', view.runActive)
  $('#btn-stop').classList.toggle('is-hidden', !view.runActive)
}

function renderCells () {
  const max = view.settings.maxDays
  const cells = visibleCells()
  if (!cells.length) {
    $('#cells').innerHTML = '<div class="empty">Ingen celler endnu. Tjek cell-manager-adressen under Indstillinger, eller tilføj en lokal celle.</div>'
    return
  }
  $('#cells').innerHTML = cells.map((c) => {
    const d = c.daysLeft
    const cls = daysClass(d)
    const sub = [
      c.accountName,
      c.block && `blok ${c.block.toUpperCase()}`,
      c.lastRenewed && `fornyet ${ago(c.lastRenewed)}`,
      c.lastInfo && `sidst tjekket: ${c.lastInfo.timeText}`
    ].filter(Boolean).join(' · ')
    const bar = Array.from({ length: max }, (_, i) => `<i class="${d != null && i < d ? 'on' : ''}"></i>`).join('')
    const removable = !c.remoteId || c.removedRemote
    return `<label class="row-item ${c.problem ? 'dim' : ''}">
      <input type="checkbox" data-cell="${esc(c.id)}" ${selected.has(c.id) ? 'checked' : ''} ${c.problem ? 'disabled' : ''}>
      <img class="head" src="${head(c.accountName)}" alt="" loading="lazy">
      <div class="grow">
        <div class="title">${esc(c.cellName)} ${c.remoteId ? '' : '<span class="tag">lokal</span>'}</div>
        <div class="sub">${esc(sub)}</div>
        ${c.problem ? `<div class="problem">${esc(c.problem)}</div>` : ''}
        <div class="daybar ${cls}">${bar}</div>
      </div>
      <span class="days ${cls}" title="Dage tilbage ifølge cell-manager">${d ?? '?'}</span>
      ${removable ? `<button class="btn small ghost" data-remove-cell="${esc(c.id)}" title="Fjern celle">✕</button>` : ''}
    </label>`
  }).join('')
}

function renderAccounts () {
  const running = new Set((view.run?.items || []).filter((i) => i.state === 'running').map((i) => i.accountId))
  if (!view.accounts.length) {
    $('#accounts').innerHTML = '<div class="empty">Ingen accounts endnu. Tryk "Tilføj account" og log ind med Microsoft.</div>'
    return
  }
  $('#accounts').innerHTML = view.accounts.map((a) => {
    let status = 'Klar'
    let dot = 'ok'
    if (running.has(a.id)) status = 'Logget ind (kører)'
    else if (!a.enabled) { status = 'Deaktiveret'; dot = '' } else if (!a.hasTokens) { status = 'Mangler login'; dot = 'warn' } else if (a.lastError) { status = 'Fejl ved sidste kørsel'; dot = 'bad' }
    const sub = [status, `${a.cellCount} celle(r)`, a.lastUsed && `sidst brugt ${fmtTime(a.lastUsed)}`].filter(Boolean).join(' · ')
    return `<div class="row-item ${a.enabled ? '' : 'dim'}">
      <img class="head" src="${head(a.name)}" alt="" loading="lazy">
      <div class="grow">
        <div class="title">${esc(a.label || a.name)}
          ${a.label ? `<span class="tag">${esc(a.name)}</span>` : ''}
          ${a.joinMacro ? '<span class="tag">egen join-makro</span>' : ''}</div>
        <div class="sub"><span class="dot ${dot}"></span> ${esc(sub)}</div>
        ${a.lastError ? `<div class="error">${esc(a.lastError)}</div>` : ''}
      </div>
      <div class="row-actions">
        <button class="btn small" data-login="${esc(a.id)}">${a.hasTokens ? 'Log ind igen' : 'Log ind'}</button>
        <button class="btn small ghost" data-edit="${esc(a.id)}">Redigér</button>
      </div>
    </div>`
  }).join('')
}

// Samme regel som i Rust: dage rundet op, maks 9 (cell-manager tillader 0-9).
function managerDays (info) {
  if (!info) return null
  return Math.min(9, Math.ceil((info.days * 1440 + info.hours * 60 + info.minutes) / 1440))
}

const STATE_ICON = { pending: '·', running: '…', done: '✓', warning: '!', failed: '✕', cancelled: '–' }

function renderRun () {
  const run = view.run
  const items = run?.items || []
  const count = (s) => items.filter((i) => i.state === s).length
  const finished = items.length - count('pending') - count('running')

  let title = 'Ingen kørsel endnu'
  if (run && view.runActive) title = `Kører … ${finished}/${items.length} færdige`
  else if (run) {
    const parts = [`${count('done')} fulde`]
    if (count('warning')) parts.push(`${count('warning')} advarsler`)
    if (count('failed')) parts.push(`${count('failed')} fejl`)
    const label = { interrupted: 'Afbrudt', cancelled: 'Stoppet' }[run.state] || 'Færdig'
    title = `${label} ${fmtTime(run.finishedAt || run.startedAt)}: ${parts.join(', ')}`
  }
  $('#run-title').textContent = title

  const problems = count('failed') + count('warning')
  const badge = $('#run-badge')
  badge.textContent = view.runActive ? count('running') : problems
  badge.classList.toggle('is-hidden', !(view.runActive || problems))

  const retryable = items.filter((i) => ['failed', 'warning', 'cancelled', 'pending'].includes(i.state))
  $('#btn-retry-failed').classList.toggle('is-hidden', view.runActive || !retryable.length)

  if (!items.length) {
    $('#run-items').innerHTML = '<div class="empty">Tryk "Tilføj dage" for at forny de valgte celler.</div>'
    return
  }

  $('#run-items').innerHTML = items.map((i) => {
    const info = i.before
      ? `${i.before.timeText} → ${i.after ? i.after.timeText : '…'}`
      : ''
    const sub = [i.accountName, info, i.clicks ? `${i.clicks} klik` : '', i.attempts > 1 ? `forsøg ${i.attempts}` : '']
      .filter(Boolean).join(' · ')
    const pct = i.progress ? Math.round((i.progress[0] / Math.max(1, i.progress[1])) * 100) : 0
    const alertCls = i.state === 'failed' ? '' : 'warn'
    const days = managerDays(i.after || i.before)

    let sync = ''
    if (i.sync === 'done') sync = '<div class="sub">✓ cell-manager opdateret</div>'
    else if (i.sync === 'notLinked') sync = '<div class="sub">Lokal celle – ikke sendt til cell-manager</div>'
    else if (i.sync === 'failed') sync = `<div class="alert">cell-manager fejlede: ${esc(i.syncError)} <button class="link" data-sync="${esc(i.cellId)}">Prøv igen</button></div>`
    else if (i.sync === 'needsConfirm' && days != null) sync = `<div class="row"><button class="btn small" data-sync="${esc(i.cellId)}">Send ${days} dage til cell-manager</button></div>`

    const canRetry = !view.runActive && ['failed', 'warning', 'cancelled'].includes(i.state)
    return `<div class="row-item run-item ${esc(i.state)}">
      <span class="state ${esc(i.state)}">${STATE_ICON[i.state] || '?'}</span>
      <div class="grow">
        <div class="title">${esc(i.cellName)} <span class="sub">${esc(i.step)}</span></div>
        <div class="sub">${esc(sub)}</div>
        ${i.state === 'running' && i.progress ? `<div class="progress"><div data-pct="${pct}"></div></div>` : ''}
        ${i.error ? `<div class="alert ${alertCls}">${esc(i.error)}</div>` : ''}
        ${sync}
      </div>
      <div class="row-actions">
        ${canRetry ? `<button class="btn small" data-retry="${esc(i.cellId)}">Prøv igen</button>` : ''}
        <button class="btn small ghost" data-log="${esc(i.cellId)}">Log</button>
      </div>
    </div>`
  }).join('')
  // CSP tillader ikke inline style-attributter, så bredden sættes via CSSOM.
  $$('.progress > div').forEach((el) => { el.style.width = `${el.dataset.pct}%` })
}

function logHtml (l, cellName) {
  return `<div class="lv-${esc(l.level)}"><span class="t">${esc(l.time)}</span> ${cellName ? `<span class="c">${esc(cellName)}</span> ` : ''}${esc(l.text)}</div>`
}

function appendLog ({ cellName, line }) {
  logLines.push({ cellName, line })
  const el = $('#log')
  const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40
  el.insertAdjacentHTML('beforeend', logHtml(line, cellName))
  if (logLines.length > 500) {
    logLines.splice(0, 100)
    for (let i = 0; i < 100 && el.firstChild; i++) el.removeChild(el.firstChild)
  }
  if (atBottom) el.scrollTop = el.scrollHeight
}

// ── Faner ─────────────────────────────────────────────────────────────────

function setTab (name) {
  if (name === 'run' && isWide()) name = 'cells'
  tab = name
  $$('.tab').forEach((b) => b.classList.toggle('active', b.dataset.tab === name))
  $('#panel-cells').classList.toggle('is-hidden', name !== 'cells')
  $('#panel-accounts').classList.toggle('is-hidden', name !== 'accounts')
  $('#panel-run').classList.toggle('is-hidden', name !== 'run')
}

$$('.tab').forEach((b) => b.addEventListener('click', () => setTab(b.dataset.tab)))
window.addEventListener('resize', () => { if (tab === 'run' && isWide()) setTab('cells') })

// ── Celler ────────────────────────────────────────────────────────────────

$('#cells').addEventListener('change', (e) => {
  const id = e.target.dataset.cell
  if (!id) return
  if (e.target.checked) selected.add(id)
  else selected.delete(id)
  renderRunbar()
})

$('#cells').addEventListener('click', async (e) => {
  const btn = e.target.closest('[data-remove-cell]')
  if (!btn) return
  e.preventDefault()
  setView(await call('remove_cell', { id: btn.dataset.removeCell }))
})

$('#sel-all').addEventListener('click', () => { visibleCells().filter(canRun).forEach((c) => selected.add(c.id)); render() })
$('#sel-none').addEventListener('click', () => { visibleCells().forEach((c) => selected.delete(c.id)); render() })
$('#sel-low').addEventListener('click', () => {
  visibleCells().forEach((c) => (canRun(c) && needsDays(c) ? selected.add(c.id) : selected.delete(c.id)))
  render()
})
$('#acct-filter').addEventListener('change', (e) => { filter = e.target.value; render() })

$('#btn-add-cell').addEventListener('click', () => {
  $('#cell-name').value = ''
  $('#cell-account').value = ''
  $('#account-names').innerHTML = view.accounts.map((a) => `<option value="${esc(a.name)}">`).join('')
  $('#dlg-cell').showModal()
})
$('#btn-cell-cancel').addEventListener('click', () => $('#dlg-cell').close())
$('#btn-cell-save').addEventListener('click', async () => {
  setView(await call('add_local_cell', { cellName: $('#cell-name').value, accountName: $('#cell-account').value }))
  $('#dlg-cell').close()
})

// ── Kørsel ────────────────────────────────────────────────────────────────

$('#btn-run').addEventListener('click', async () => {
  const ids = chosenIds()
  if (!ids.length) return
  setView(await call('start_run', { cellIds: ids }))
  appendLog({ cellName: '', line: { time: new Date().toLocaleTimeString('da-DK'), level: 'info', text: `── Ny kørsel: ${ids.length} celle(r) ──` } })
  if (!isWide()) setTab('run')
})

$('#btn-stop').addEventListener('click', async () => {
  await call('cancel_run')
  toast('Stopper efter det nuværende trin og logger ud …')
})

async function retry (ids) {
  setView(await call('retry_items', { cellIds: ids }))
  if (!isWide()) setTab('run')
}

$('#btn-retry-failed').addEventListener('click', () => {
  const ids = (view.run?.items || []).filter((i) => ['failed', 'warning', 'cancelled', 'pending'].includes(i.state)).map((i) => i.cellId)
  retry(ids)
})
$('#btn-resume').addEventListener('click', () => {
  const ids = (view.run?.items || []).filter((i) => ['pending', 'failed', 'cancelled'].includes(i.state)).map((i) => i.cellId)
  retry(ids)
})

$('#run-items').addEventListener('click', async (e) => {
  const t = e.target.closest('button')
  if (!t) return
  if (t.dataset.retry) retry([t.dataset.retry])
  if (t.dataset.sync) setView(await call('confirm_sync', { cellId: t.dataset.sync }))
  if (t.dataset.log) {
    const item = view.run.items.find((i) => i.cellId === t.dataset.log)
    const lines = await call('item_log', { cellId: t.dataset.log })
    $('#itemlog-title').textContent = `Log: ${item ? item.cellName : ''}`
    $('#itemlog').innerHTML = lines.map((l) => logHtml(l, '')).join('') || '<div class="t">Ingen linjer</div>'
    $('#dlg-log').showModal()
    $('#itemlog').scrollTop = $('#itemlog').scrollHeight
  }
})
$('#btn-itemlog-close').addEventListener('click', () => $('#dlg-log').close())
$('#log-clear').addEventListener('click', () => { logLines.length = 0; $('#log').innerHTML = '' })

// ── Accounts og login ────────────────────────────────────────────────────

const LOGIN_INTRO = $('#login-intro').innerHTML

function resetLoginDialog (title) {
  $('#login-title').textContent = title
  $('#login-intro').innerHTML = LOGIN_INTRO
  $('#login-intro').classList.remove('is-hidden')
  $('#login-code').classList.add('is-hidden')
  $('#btn-login-start').classList.remove('is-hidden')
  $('#btn-login-start').disabled = false
}

function openLogin (accountId) {
  const acc = view.accounts.find((a) => a.id === accountId)
  loginFor = accountId || null
  resetLoginDialog(acc ? `Log ind: ${acc.label || acc.name}` : 'Tilføj account')
  $('#dlg-login').showModal()
}

$('#btn-add-account').addEventListener('click', () => openLogin(null))
$('#accounts').addEventListener('click', (e) => {
  const t = e.target.closest('button')
  if (!t) return
  if (t.dataset.login) openLogin(t.dataset.login)
  if (t.dataset.edit) openEdit(t.dataset.edit)
})

$('#btn-login-start').addEventListener('click', async () => {
  loginBusy = true
  $('#btn-login-start').disabled = true
  $('#login-intro').innerHTML = '<p class="hint"><span class="spinner"></span> Starter login …</p>'
  try {
    setView(await call('login_account', { accountId: loginFor }))
    $('#dlg-login').close()
    toast('Account er logget ind og klar', 'ok')
  } catch {
    resetLoginDialog($('#login-title').textContent)
    $('#login-intro').insertAdjacentHTML('afterbegin', '<p class="error">Login lykkedes ikke. Prøv igen.</p>')
  } finally {
    loginBusy = false
  }
})

$('#btn-login-cancel').addEventListener('click', async () => {
  if (loginBusy) await call('cancel_login').catch(() => {})
  $('#dlg-login').close()
})

$('#btn-copy-code').addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText($('#code').textContent)
    toast('Koden er kopieret', 'ok')
  } catch {
    toast('Kunne ikke kopiere. Markér koden og kopiér selv.')
  }
})
$('#btn-open-link').addEventListener('click', () => call('open_url', { url: codeUrl }))

function showCode (p) {
  const dlg = $('#dlg-login')
  if (p.context === 'run') {
    resetLoginDialog(`Log ind igen: ${p.accountName}`)
    $('#login-code-text').textContent = `Login for ${p.accountName} er udløbet. Gå til siden og skriv koden:`
  } else {
    $('#login-code-text').textContent = 'Gå til siden, skriv koden og log ind med den Microsoft-konto, der ejer Minecraft:'
  }
  $('#login-intro').classList.add('is-hidden')
  $('#btn-login-start').classList.add('is-hidden')
  $('#login-code').classList.remove('is-hidden')
  $('#code').textContent = p.code
  codeUrl = p.url.includes('?') ? p.url : `${p.url}?otc=${encodeURIComponent(p.code)}`
  if (!dlg.open) dlg.showModal()
}

// ── Redigér account ──────────────────────────────────────────────────────

function openEdit (id) {
  const a = view.accounts.find((x) => x.id === id)
  if (!a) return
  editId = id
  $('#acct-title').textContent = a.name
  $('#acct-label').value = a.label || ''
  $('#acct-enabled').checked = a.enabled
  $('#acct-join').value = a.joinMacro || ''
  $('#acct-join-errors').textContent = ''
  const rm = $('#btn-acct-remove')
  rm.textContent = 'Fjern account'
  delete rm.dataset.armed
  $('#dlg-account').showModal()
}

$('#btn-acct-cancel').addEventListener('click', () => $('#dlg-account').close())
$('#btn-acct-save').addEventListener('click', async () => {
  const patch = { label: $('#acct-label').value, enabled: $('#acct-enabled').checked, joinMacro: $('#acct-join').value }
  setView(await call('update_account', { id: editId, patch }))
  $('#dlg-account').close()
})
$('#btn-acct-remove').addEventListener('click', async (e) => {
  const btn = e.currentTarget
  if (!btn.dataset.armed) {
    btn.dataset.armed = '1'
    btn.textContent = 'Klik igen for at fjerne'
    return
  }
  setView(await call('remove_account', { id: editId }))
  $('#dlg-account').close()
})

// ── Makro-validering ─────────────────────────────────────────────────────

function liveValidate (textarea, out) {
  let timer = null
  textarea.addEventListener('input', () => {
    clearTimeout(timer)
    timer = setTimeout(async () => {
      const errors = await invoke('validate_macro', { script: textarea.value }).catch(() => [])
      out.textContent = errors.join(' · ')
    }, 300)
  })
}
liveValidate($('#acct-join'), $('#acct-join-errors'))
$$('textarea[data-key]').forEach((t) => liveValidate(t, $(`[data-errors="${t.dataset.key}"]`)))

// ── Indstillinger ────────────────────────────────────────────────────────

function fillSettings (s) {
  $$('#dlg-settings [data-key]').forEach((el) => {
    const v = s[el.dataset.key]
    if (el.type === 'checkbox') el.checked = !!v
    else el.value = v ?? ''
  })
  $$('[data-errors]').forEach((el) => { el.textContent = '' })
}

$('#btn-settings').addEventListener('click', () => {
  fillSettings(view.settings)
  const key = $('#api-key')
  key.value = ''
  key.placeholder = view.apiKeySet ? 'Gemt – skriv en ny for at skifte' : 'Ingen'
  $('#test-mgr-result').textContent = ''
  $('#data-dir').textContent = `Data gemmes i: ${view.dataDir}`
  $('#dlg-settings').showModal()
})
$('#btn-settings-cancel').addEventListener('click', () => $('#dlg-settings').close())

function collectSettings () {
  const s = { ...view.settings }
  $$('#dlg-settings [data-key]').forEach((el) => {
    const k = el.dataset.key
    if (el.type === 'checkbox') s[k] = el.checked
    else if (el.type === 'number') s[k] = Number(el.value)
    else s[k] = el.value
  })
  return s
}

$('#btn-settings-save').addEventListener('click', async () => {
  const apiKey = $('#api-key').value.trim()
  setView(await call('save_settings', { settings: collectSettings(), apiKey: apiKey || null }))
  $('#dlg-settings').close()
  toast('Indstillinger gemt', 'ok')
  call('sync_cells').then(setView).catch(() => {})
})

$('#btn-reset-macros').addEventListener('click', async () => {
  const d = await call('default_settings')
  $('[data-key="joinMacro"]').value = d.joinMacro
  $('[data-key="renewMacro"]').value = d.renewMacro
})

$('#btn-test-mgr').addEventListener('click', async () => {
  const out = $('#test-mgr-result')
  out.textContent = 'Tester …'
  try {
    out.textContent = await invoke('test_manager', {
      url: $('[data-key="managerUrl"]').value,
      apiKey: $('#api-key').value.trim() || null
    })
  } catch (e) {
    out.textContent = String(e)
  }
})

// ── Eksport / import ─────────────────────────────────────────────────────

function openTransfer (mode) {
  transferMode = mode
  $('#transfer-title').textContent = mode === 'export' ? 'Eksportér accounts' : 'Importér accounts'
  $('#transfer-hint').textContent = mode === 'export'
    ? 'Vælg et kodeord på mindst 8 tegn. Du skal bruge det, når du importerer på den anden PC.'
    : 'Skriv kodeordet fra eksporten. Importerer du headless-clients accounts.json, kan feltet være tomt.'
  $('#transfer-pw').value = ''
  $('#dlg-transfer').showModal()
}
$('#btn-export').addEventListener('click', () => openTransfer('export'))
$('#btn-import').addEventListener('click', () => openTransfer('import'))
$('#btn-transfer-cancel').addEventListener('click', () => $('#dlg-transfer').close())
$('#btn-transfer-go').addEventListener('click', async () => {
  const password = $('#transfer-pw').value
  const cmd = transferMode === 'export' ? 'export_accounts' : 'import_accounts'
  const msg = await call(cmd, { password })
  $('#dlg-transfer').close()
  toast(msg, 'ok')
  setView(await call('get_view'))
})

// ── Øvrigt ───────────────────────────────────────────────────────────────

$('#mgr').addEventListener('click', () => call('sync_cells').then(setView).catch(() => {}))
$('#notice-close').addEventListener('click', async () => setView(await call('dismiss_notice')))

listen('view', (e) => setView(e.payload))
listen('run', (e) => {
  if (!view) return
  view.run = e.payload
  renderRun()
  renderAccounts()
  renderRunbar()
  renderTop()
})
listen('log', (e) => appendLog(e.payload))
listen('msa-code', (e) => showCode(e.payload))

setInterval(() => { if (view) { renderTop(); renderCells() } }, 60000)

invoke('get_view').then(setView)
