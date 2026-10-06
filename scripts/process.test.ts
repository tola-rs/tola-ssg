// Every case here drives a real operating-system process tree: output capture, the deadline, and
// the termination a caller depends on when it cancels. The child command is `deno eval` because it
// needs no file on disk and grants the child the permissions its snippet uses.
//
// A case bounds itself through the deadline it passes to `runProcess` — the production feature it
// is testing — because the test runner's own suite has no per-test deadline to set.

import { expect } from '@std/expect'
import { describe, test } from '@std/testing/bdd'
import { runProcess } from './process.ts'

const RUNTIME = Deno.execPath()

function terminateDescendant(pid: number): void {
  try {
    process.kill(pid, 'SIGKILL')
  } catch (error) {
    if (!(error instanceof Error && 'code' in error && error.code === 'ESRCH')) throw error
  }
}

describe('subprocesses', () => {
  test('retains both output streams and the command exit status', async () => {
    const output = await runProcess(RUNTIME, [
      'eval',
      'process.stdout.write("output"); process.stderr.write("diagnostic"); process.exitCode = 7',
    ])
    expect(output).toEqual({ stdout: 'output', stderr: 'diagnostic', exitCode: 7, timedOut: false })
  })

  test('reports a missing executable as a failed command', async () => {
    const output = await runProcess('/tola-missing-command/does-not-exist', [])
    expect(output.exitCode).toBe(127)
    expect(output.timedOut).toBe(false)
  })

  test('an already aborted operation never starts a command', async () => {
    const reason = new Error('cancelled before execution')
    const signal = AbortSignal.abort(reason)
    await expect(runProcess('/tola-missing-command/does-not-exist', [], { signal })).rejects.toBe(reason)
  })

  test('strict decoding preserves a byte order mark', async () => {
    const output = await runProcess(RUNTIME, ['eval', 'process.stdout.write("\\uFEFF雪")'], {
      strictUtf8: true,
    })
    expect(output.stdout).toBe('\uFEFF雪')
  })

  test('strict decoding rejects malformed output', async () => {
    await expect(
      runProcess(RUNTIME, ['eval', 'Deno.stdout.writeSync(Uint8Array.of(0xff))'], { strictUtf8: true }),
    ).rejects.toThrow()
  })

  test('inherited progress reaches the consumer before a command can finish', async () => {
    const release = Promise.withResolvers<Response>()
    const server = Deno.serve({
      hostname: '127.0.0.1',
      port: 0,
      onListen: () => {},
      handler: () => release.promise,
    })
    const address = `http://127.0.0.1:${server.addr.port}/`
    const command = `
      process.stdout.write('o');
      process.stderr.write('e');
      await fetch(${JSON.stringify(address)});
      process.exitCode = 7;
    `
    const source = `
      import { runProcess } from ${JSON.stringify(new URL('./process.ts', import.meta.url).href)};
      const output = await runProcess(Deno.execPath(), ['eval', ${JSON.stringify(command)}], {
        stdout: 'inherit', stderr: 'inherit', timeoutMs: 5_000
      });
      process.exitCode = output.exitCode;
    `
    const child = new Deno.Command(RUNTIME, {
      args: ['eval', source],
      stdout: 'piped',
      stderr: 'piped',
    }).spawn()
    const stdout = child.stdout.getReader()
    const stderr = child.stderr.getReader()
    try {
      const [out, diagnostic] = await Promise.all([stdout.read(), stderr.read()])
      expect(new TextDecoder().decode(out.value)).toBe('o')
      expect(new TextDecoder().decode(diagnostic.value)).toBe('e')
      release.resolve(new Response('finish'))
      expect((await child.status).code).toBe(7)
    } finally {
      release.resolve(new Response('finish'))
      await child.status
      stdout.releaseLock()
      stderr.releaseLock()
      await server.shutdown()
    }
  })

  test('cancellation terminates descendants that ignore SIGTERM after closing their output', async () => {
    interface Descendant {
      pid: number
      url: string
    }
    const ready = Promise.withResolvers<Descendant>()
    const connected = Promise.withResolvers<void>()
    const monitor = Deno.serve({
      hostname: '127.0.0.1',
      port: 0,
      onListen: () => {},
      async handler(request) {
        if (new URL(request.url).pathname === '/ready') ready.resolve((await request.json()) as Descendant)
        else connected.resolve()
        return new Response('ready')
      },
    })
    const monitorUrl = `http://127.0.0.1:${monitor.addr.port}/`
    // The descendant ignores SIGTERM and holds an accepted connection; only the tree's second
    // signal can end it, and closing that connection is what proves the descendant died.
    const descendantSource = `
      process.on('SIGTERM', () => {});
      const server = Deno.serve({ hostname: '127.0.0.1', port: 0, onListen: () => {}, handler: async () => {
        await fetch(${JSON.stringify(`${monitorUrl}connected`)});
        return await Promise.withResolvers().promise;
      }});
      await fetch(${JSON.stringify(`${monitorUrl}ready`)}, {
        method: 'POST',
        body: JSON.stringify({ pid: process.pid, url: \`http://127.0.0.1:\${server.addr.port}/\` })
      });
    `
    const source = `
      const child = new Deno.Command(Deno.execPath(), {
        args: ['eval', ${JSON.stringify(descendantSource)}], stdin: 'null', stdout: 'null', stderr: 'null'
      }).spawn();
      await child.status;
    `
    const controller = new AbortController()
    const reason = new Error('cancel process tree')
    const operation = runProcess(RUNTIME, ['eval', source], {
      signal: controller.signal,
      stdout: 'inherit',
      stderr: 'inherit',
      timeoutMs: 10_000,
    })
    let descendant: Descendant | undefined
    let disconnected: Promise<boolean> | undefined
    try {
      descendant = await Promise.race([
        ready.promise,
        operation.then(() => {
          throw new Error('command exited before its descendant became ready')
        }),
      ])
      // The OS must close an accepted connection; a real deadline only bounds a leaked child.
      const deadline = AbortSignal.timeout(5_000)
      disconnected = fetch(descendant.url, { signal: deadline }).then(
        () => false,
        () => !deadline.aborted,
      )
      await Promise.race([
        connected.promise,
        disconnected.then(() => {
          throw new Error('descendant did not keep the connection open')
        }),
      ])
      controller.abort(reason)
      await expect(operation).rejects.toBe(reason)
      expect(await disconnected).toBe(true)
    } finally {
      controller.abort(reason)
      await operation.catch(() => {})
      if (descendant !== undefined) terminateDescendant(descendant.pid)
      await disconnected
      await monitor.shutdown()
    }
  })

  test('a deadline terminates a running command', async () => {
    const output = await runProcess(RUNTIME, ['eval', 'setInterval(() => {}, 1000)'], {
      timeoutMs: 50,
    })
    expect(output.exitCode).toBe(124)
    expect(output.timedOut).toBe(true)
  })

  test('a POSIX group retains descendants after the parent exits', {
    ignore: process.platform === 'win32',
  }, async () => {
    // The descendant outlives its root, so only a signal to the group can reach it.
    const source = `
      const child = new Deno.Command(Deno.execPath(), {
        args: ['eval', 'setInterval(() => {}, 1000)'], stdout: 'inherit', stderr: 'inherit'
      }).spawn();
      child.unref();
      process.stdout.write('parent finished');
      process.exit(0);
    `
    const output = await runProcess(RUNTIME, ['eval', source], { timeoutMs: 500 })
    expect(output.stdout).toBe('parent finished')
    expect(output.exitCode).toBe(124)
    expect(output.timedOut).toBe(true)
  })

  test('a POSIX interrupt is confined to its command scope', {
    ignore: process.platform === 'win32',
  }, async () => {
    const moduleUrl = new URL('./process.ts', import.meta.url).href
    const cancellationUrl = new URL('./cancellation.ts', import.meta.url).href
    // Each command carries a deadline: a broken interrupt then fails the assertions below instead
    // of waiting on a process nothing will stop.
    const source = `
      import { runProcess } from ${JSON.stringify(moduleUrl)};
      import { withCancellation, CommandCancelled } from ${JSON.stringify(cancellationUrl)};
      let cancelledCode;
      try {
        await withCancellation(async signal => {
          const first = runProcess(Deno.execPath(), ['eval', 'setInterval(() => {}, 1000)'], {
            signal, timeoutMs: 5_000
          });
          queueMicrotask(() => process.kill(process.pid, 'SIGINT'));
          await first;
          console.log('unexpected continuation');
        });
      } catch (error) {
        if (!(error instanceof CommandCancelled)) throw error;
        cancelledCode = error.exitCode;
      }
      const subsequent = await withCancellation(signal =>
        runProcess(Deno.execPath(), ['eval', 'console.log("next command")'], { signal })
      );
      console.log(JSON.stringify([cancelledCode, subsequent.exitCode, subsequent.stdout]));
      process.exitCode = cancelledCode;
    `
    const child = new Deno.Command(RUNTIME, {
      args: ['eval', source],
      stdout: 'piped',
      stderr: 'piped',
    }).spawn()
    const [stdout, stderr, status] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.status,
    ])
    expect(JSON.parse(stdout)).toEqual([130, 0, 'next command\n'])
    expect(stderr).toBe('')
    expect(status.code).toBe(130)
  })
})
