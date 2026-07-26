export type TerminalWriter = (sessionId: number, input: string) => Promise<void>;
export type TerminalOperation = (sessionId: number) => Promise<void>;
export const MAX_PENDING_INPUT_CODE_UNITS = 64 * 1024;

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

    let offset = 0;
    while (offset < input.length) {
      if (!this.pending) this.pending = { sessionId, input: '' };

      const capacity = MAX_PENDING_INPUT_CODE_UNITS - this.pending.input.length;
      if (capacity <= 0) {
        this.flush();
        continue;
      }

      let end = Math.min(input.length, offset + capacity);
      if (
        end < input.length
        && end > offset
        && /[\uD800-\uDBFF]/u.test(input[end - 1])
        && /[\uDC00-\uDFFF]/u.test(input[end])
      ) {
        end -= 1;
      }
      if (end === offset) {
        this.flush();
        continue;
      }

      this.pending.input += input.slice(offset, end);
      offset = end;
      if (this.pending.input.length >= MAX_PENDING_INPUT_CODE_UNITS) this.flush();
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
