#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

function options(argv) {
  const result = { timeout: 10000, target: [] };
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i];
    if (flag === '--') { result.target = argv.slice(i + 1); break; }
    if (flag === '--help') {
      console.log('inspect.mjs [--break FILE:LINE] [--expression JS] [--timeout-ms N] -- TARGET [ARGS]');
      process.exit(0);
    }
    const value = argv[++i];
    if (value === undefined) throw new Error(`missing value for ${flag}`);
    if (flag === '--break') {
      const match = /^(.*):([1-9][0-9]*)$/.exec(value);
      if (!match) throw new Error('--break requires FILE:LINE (1-based)');
      result.breakpoint = { url: pathToFileURL(resolve(match[1])).href, lineNumber: Number(match[2]) - 1 };
    } else if (flag === '--expression') result.expression = value;
    else if (flag === '--timeout-ms') result.timeout = Number(value);
    else throw new Error(`unknown option ${flag}`);
  }
  if (!result.target.length) throw new Error('provide a target after --');
  if (!Number.isSafeInteger(result.timeout) || result.timeout < 1 || result.timeout > 120000)
    throw new Error('--timeout-ms must be 1..120000');
  if (typeof WebSocket !== 'function') throw new Error('global WebSocket unavailable; use Node 22.4+');
  return result;
}

function emit(type, details) {
  process.stderr.write(`${JSON.stringify({ type, ...details })}\n`);
}

async function main() {
  const args = options(process.argv.slice(2));
  const child = spawn(process.execPath, ['--inspect-brk=127.0.0.1:0', resolve(args.target[0]), ...args.target.slice(1)], {
    stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true,
  });
  let ws, nextId = 0, stderr = '', connected = false, failure = null;
  const pending = new Map();
  const fail = (error) => {
    if (!failure) {
      failure = error.message;
      emit('debugger_error', { error: failure });
    }
    ws?.close();
    child.kill();
  };
  const timer = setTimeout(() => fail(new Error('Inspector deadline exceeded')), args.timeout);
  const signal = () => fail(new Error('Inspector interrupted'));
  process.once('SIGINT', signal);
  process.once('SIGTERM', signal);
  child.stdout.on('data', (data) => process.stdout.write(data));

  function post(method, params = {}) {
    return new Promise((accept, reject) => {
      const id = ++nextId;
      pending.set(id, { accept, reject });
      try { ws.send(JSON.stringify({ id, method, params })); }
      catch (error) { pending.delete(id); reject(error); }
    });
  }

  async function paused(params) {
    emit('paused', { reason: params.reason, frames: params.callFrames.slice(0, 20).map((frame) => ({
      name: frame.functionName, url: frame.url, scriptId: frame.location.scriptId,
      line: frame.location.lineNumber + 1, column: frame.location.columnNumber + 1,
    })) });
    try {
      if (args.expression !== undefined && params.reason !== 'Break on start' && params.callFrames.length) {
        const result = await post('Debugger.evaluateOnCallFrame', {
          callFrameId: params.callFrames[0].callFrameId, expression: args.expression,
          returnByValue: false, generatePreview: false, silent: true,
        });
        const value = result.result;
        emit('evaluation', { value: value ? {
          type: value.type, value: typeof value.value === 'string' ? value.value.slice(0, 4096) : value.value,
          description: value.description?.slice(0, 4096), unserializableValue: value.unserializableValue,
        } : null, exception: result.exceptionDetails?.text });
      }
    } finally { await post('Debugger.resume'); }
  }

  async function connect(url) {
    const address = new URL(url);
    if (address.hostname !== '127.0.0.1') throw new Error('expected loopback Inspector endpoint');
    ws = new WebSocket(url);
    ws.addEventListener('message', (event) => {
      try {
        const message = JSON.parse(event.data);
        if (message.id) {
          const promise = pending.get(message.id);
          pending.delete(message.id);
          if (message.error) promise?.reject(new Error(message.error.message));
          else promise?.accept(message.result);
        } else if (message.method === 'Debugger.paused') paused(message.params).catch(fail);
      } catch (error) { fail(error); }
    });
    ws.addEventListener('close', () => {
      for (const promise of pending.values()) promise.reject(new Error('Inspector connection closed'));
      pending.clear();
    });
    await new Promise((accept, reject) => {
      ws.addEventListener('open', accept, { once: true });
      ws.addEventListener('error', () => reject(new Error('Inspector connection failed')), { once: true });
    });
    await post('Debugger.enable');
    await post('Debugger.setPauseOnExceptions', { state: 'uncaught' });
    if (args.breakpoint) emit('breakpoint', await post('Debugger.setBreakpointByUrl', args.breakpoint));
    await post('Runtime.runIfWaitingForDebugger');
  }

  child.stderr.on('data', (data) => {
    process.stderr.write(data);
    stderr = (stderr + data.toString()).slice(-8192);
    if (!connected) {
      const match = /Debugger listening on (ws:\/\/127\.0\.0\.1:\d+\/[^\s]+)/.exec(stderr);
      if (match) { connected = true; connect(match[1]).catch(fail); }
    }
    if (stderr.includes('Waiting for the debugger to disconnect')) ws?.close();
  });
  child.on('error', fail);
  const status = await new Promise((accept) => child.once('close', (code, signal) => accept({ code, signal })));
  clearTimeout(timer);
  process.removeListener('SIGINT', signal);
  process.removeListener('SIGTERM', signal);
  ws?.close();
  emit('target_exit', { ...status, error: failure });
  process.exitCode = failure ? 1 : (status.code ?? 1);
}

main().catch((error) => { emit('debugger_error', { error: error.message }); process.exitCode = 1; });
