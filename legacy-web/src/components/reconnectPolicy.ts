export const RECONNECT_POLICY = {
  maxAttempts: 4,
  baseDelayMs: 250,
  maxDelayMs: 2000,
  stableSessionMs: 5000,
} as const;

export interface ReconnectDecision {
  attempt: number;
  delayMs: number;
}

/**
 * Returns the next bounded reconnect attempt, or null once the retry budget is
 * exhausted. A session that stayed alive long enough starts with a fresh budget.
 */
export function getReconnectDecision(
  previousAttempts: number,
  connectedForMs: number,
): ReconnectDecision | null {
  const attempts = connectedForMs >= RECONNECT_POLICY.stableSessionMs
    ? 0
    : Math.max(0, Math.trunc(previousAttempts));

  if (attempts >= RECONNECT_POLICY.maxAttempts) return null;

  return {
    attempt: attempts + 1,
    delayMs: Math.min(
      RECONNECT_POLICY.baseDelayMs * 2 ** attempts,
      RECONNECT_POLICY.maxDelayMs,
    ),
  };
}
