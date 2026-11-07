import { once } from 'node:events'
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http'
import type { Socket } from 'node:net'

export type Gate = {
  url: string
  /** The gate's server, for callers that must observe server-level errors. */
  server: Server
  /** Drops open connections instead of waiting for the responses they still hold. */
  close(): Promise<void>
}

/**
 * Holds every response until its handler releases it, so a hook command can block on the gate.
 * The handler owns its per-request state; the gate owns the socket set and teardown.
 */
export async function startGate(
  handler: (request: IncomingMessage, response: ServerResponse) => void,
): Promise<Gate> {
  const connections = new Set<Socket>()
  const server = createServer(handler)
  server.on('connection', (socket) => {
    connections.add(socket)
    socket.once('close', () => connections.delete(socket))
  })
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  const address = server.address()
  if (!address || typeof address === 'string') {
    await new Promise<void>((resolve) => server.close(() => resolve()))
    throw new Error('Gate server has no TCP address')
  }
  return {
    url: `http://127.0.0.1:${address.port}`,
    server,
    close: () =>
      new Promise<void>((resolve, reject) => {
        for (const socket of connections) socket.destroy()
        connections.clear()
        if (!server.listening) {
          resolve()
          return
        }
        server.close((error) => (error ? reject(error) : resolve()))
      }),
  }
}
