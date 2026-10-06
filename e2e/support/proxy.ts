import type { Duplex } from 'node:stream'
import { startGate } from './gate.ts'

export type RefusingProxy = {
  url: string
  /** Every request URL and CONNECT target, in arrival order. */
  requests: string[]
  close(): Promise<void>
}

export type HeldProxy = {
  url: string
  /** The tunnels CONNECT opened; destroying them releases whatever waits on them. */
  tunnels: Duplex[]
  close(): Promise<void>
}

/** Answers every request and CONNECT tunnel with 502, recording each URL and CONNECT target. */
export async function startRefusingProxy(): Promise<RefusingProxy> {
  const requests: string[] = []
  const gate = await startGate((request, response) => {
    requests.push(request.url ?? '')
    response.writeHead(502).end()
  })
  gate.server.on('connect', (request, socket) => {
    requests.push(request.url ?? '')
    socket.end('HTTP/1.1 502 Bad Gateway\r\n\r\n')
  })
  return { url: gate.url, requests, close: gate.close }
}

/** Leaves every CONNECT tunnel unanswered, so a download through the proxy cannot finish. */
export async function startHeldProxy(): Promise<HeldProxy> {
  const tunnels: Duplex[] = []
  const gate = await startGate((_request, response) => {
    response.writeHead(502).end()
  })
  gate.server.on('connect', (_request, socket) => {
    tunnels.push(socket)
  })
  return { url: gate.url, tunnels, close: gate.close }
}

/**
 * Routes a child's HTTP and HTTPS traffic through `url`, in both letter cases; `NO_PROXY` stays
 * empty so a host exemption cannot bypass the proxy.
 */
export function proxyEnvironment(url: string): NodeJS.ProcessEnv {
  return {
    HTTP_PROXY: url,
    HTTPS_PROXY: url,
    ALL_PROXY: url,
    NO_PROXY: '',
    http_proxy: url,
    https_proxy: url,
    all_proxy: url,
    no_proxy: '',
  }
}
