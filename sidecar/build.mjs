// Bygger sidecaren til én exe (Node SEA) og lægger den i src-tauri/binaries
// med det navn Tauri forventer: cellkeeper-sidecar-<target-triple>[.exe].
//
// Trin:
// 1. esbuild bundler index.js + mineflayer til én fil.
//    minecraft-data beskæres til kun 1.8 (ellers bliver exe'en 400+ MB).
// 2. Node SEA: blob injiceres i en kopi af node-binæren.
import { build } from 'esbuild'
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const outDir = path.join(here, 'build')
const binDir = path.join(here, '..', 'src-tauri', 'binaries')
fs.mkdirSync(outDir, { recursive: true })
fs.mkdirSync(binDir, { recursive: true })

// Behold kun pc/1.8-blokken fra minecraft-data/data.js.
const pruneMinecraftData = {
  name: 'prune-minecraft-data',
  setup (b) {
    b.onLoad({ filter: /minecraft-data[\\/]data\.js$/ }, (args) => {
      const lines = fs.readFileSync(args.path, 'utf8').split('\n')
      const pcStart = lines.findIndex(l => /^ {2}'pc': \{/.test(l))
      const start = lines.findIndex((l, i) => i > pcStart && /^ {4}'1\.8': \{/.test(l))
      if (pcStart < 0 || start < 0) throw new Error('Kunne ikke finde pc/1.8 i minecraft-data')
      let end = start
      while (!/^ {4}\},?$/.test(lines[end])) end++
      const block = lines.slice(start, end + 1).join('\n').replace(/\},?$/, '}')
      return {
        contents: `module.exports =\n{\n  'pc': {\n${block}\n  },\n  'bedrock': {}\n}\n`,
        loader: 'js',
        resolveDir: path.dirname(args.path)
      }
    })
  }
}

const bundlePath = path.join(outDir, 'sidecar.js')
await build({
  entryPoints: [path.join(here, 'index.js')],
  bundle: true,
  platform: 'node',
  target: 'node22',
  minify: true,
  outfile: bundlePath,
  loader: { '.node': 'empty' },
  plugins: [pruneMinecraftData],
  logLevel: 'warning'
})
console.log(`bundle: ${(fs.statSync(bundlePath).size / 1e6).toFixed(1)} MB`)

// Node SEA
const seaConfig = path.join(outDir, 'sea-config.json')
const blob = path.join(outDir, 'sea-prep.blob')
fs.writeFileSync(seaConfig, JSON.stringify({
  main: bundlePath,
  output: blob,
  disableExperimentalSEAWarning: true
}))
execFileSync(process.execPath, ['--experimental-sea-config', seaConfig], { stdio: 'inherit' })

const triple = /host: (\S+)/.exec(execFileSync('rustc', ['-vV']).toString())[1]
const isWin = process.platform === 'win32'
const isMac = process.platform === 'darwin'
const target = path.join(binDir, `cellkeeper-sidecar-${triple}${isWin ? '.exe' : ''}`)
fs.copyFileSync(process.execPath, target)
fs.chmodSync(target, 0o755)

if (isMac) execFileSync('codesign', ['--remove-signature', target], { stdio: 'inherit' })

const postject = path.join(here, 'node_modules', 'postject', 'dist', 'cli.js')
const postjectArgs = [postject, target, 'NODE_SEA_BLOB', blob,
  '--sentinel-fuse', 'NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2']
if (isMac) postjectArgs.push('--macho-segment-name', 'NODE_SEA')
execFileSync(process.execPath, postjectArgs, { stdio: 'inherit' })

if (isMac) execFileSync('codesign', ['--sign', '-', target], { stdio: 'inherit' })

console.log(`sidecar: ${target} (${(fs.statSync(target).size / 1e6).toFixed(0)} MB)`)
