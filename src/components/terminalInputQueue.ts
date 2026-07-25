export type TerminalWriter = (sessionId: number, input: string) => Promise<void>;
export type TerminalOperation = (sessionId: number) => Promise<void>;

interface PendingInput {
  sessionId: number;
  input: string;
}

/**
 * Preserves the exact order of terminal input across the asynchronous Tauri
 * bridge. Keystrokes emitted in the same JavaScript turn are batched, while
 * later writes and control operations are serialized behind earlier ones.
 */
export class TerminalInputQueue {
  private pending: PendingInput | null = null;
  private chain: Promise<void> = Promise.resolve();
  private flushScheduled = false;
  private disposed = false;
  private readonly writer: TerminalWriter;
  private readonly onError: (error: unknown) => void;

  constructor(
    writer: TerminalWriter,
    onError: (error: unknown) => void = console.error,
  ) {
    this.writer = writer;
    this.onError = onError;
  }

  enqueue(sessionId: number, input: string) {
    if (this.disposed || !input) return;

    if (this.pending && this.pending.sessionId !== sessionId) {
      this.flush();
    }

    if (this.pending) {
      this.pending.input += input;
    } else {
      this.pending = { sessionId, input };
    }

    if (!this.flushScheduled) {
      this.flushScheduled = true;
      queueMicrotask(() => {
        this.flushScheduled = false;
        this.flush();
      });
    }
  }

  enqueueOperation(sessionId: number, operation: TerminalOperation) {
    if (this.disposed) return;
    this.flush();
    this.append(() => operation(sessionId));
  }

  flush(): Promise<void> {
    if (this.disposed || !this.pending) return this.chain;

    const { sessionId, input } = this.pending;
    this.pending = null;
    this.append(() => this.writer(sessionId, input));
    return this.chain;
  }

  async whenIdle() {
    await this.flush();
    await this.chain;
  }

  dispose() {
    this.disposed = true;
    this.pending = null;
  }

  private append(operation: () => Promise<void>) {
    this.chain = this.chain
      .then(async () => {
        if (!this.disposed) await operation();
      })
      .catch(this.onError);
  }
}
