export class CommandCancelled extends Error {
  readonly exitCode: number
  readonly signal: 'SIGINT' | 'SIGTERM'

  constructor(signal: 'SIGINT' | 'SIGTERM', cause?: unknown) {
    const detail = cause instanceof Error ? cause.message : cause === undefined ? '' : String(cause)
    super(
      `operation cancelled by ${signal}${detail.length === 0 ? '' : `\n${detail}`}`,
      cause === undefined ? undefined : { cause },
    )
    this.name = 'CommandCancelled'
    this.signal = signal
    this.exitCode = signal === 'SIGINT' ? 130 : 143
  }
}

/** Keep signal handling active until the operation has finished its cleanup. */
export async function withCancellation<T>(operation: (signal: AbortSignal) => Promise<T>): Promise<T> {
  const controller = new AbortController()
  const onInterrupt = () => controller.abort(new CommandCancelled('SIGINT'))
  const onTerminate = () => controller.abort(new CommandCancelled('SIGTERM'))
  process.on('SIGINT', onInterrupt)
  process.on('SIGTERM', onTerminate)
  try {
    const value = await operation(controller.signal)
    controller.signal.throwIfAborted()
    return value
  } catch (error) {
    if (controller.signal.aborted) {
      const reason: unknown = controller.signal.reason
      if (
        reason instanceof CommandCancelled &&
        error !== reason &&
        !(error instanceof Error && error.name === 'AbortError')
      ) {
        throw new CommandCancelled(reason.signal, error)
      }
      throw reason
    }
    throw error
  } finally {
    process.off('SIGINT', onInterrupt)
    process.off('SIGTERM', onTerminate)
  }
}
