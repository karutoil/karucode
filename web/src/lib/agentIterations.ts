/**
 * Keep a live iteration count from falling backwards when a fetch lands.
 *
 * `iterations` only grows, but the server row is written between iterations, so
 * a refresh can return the count this page already exceeded - or the initial `0`
 * for a run that has only just started. Replacing the roster wholesale made the
 * number visibly drop (and made a reload show zero for every running subagent),
 * so every merge keeps the larger of the two. Membership still follows
 * `incoming`: this only rewrites the count.
 *
 * Lives outside `state/agentConsole.ts` because `state/chat.ts` needs the same
 * rule when it reseeds a session snapshot, and `chat.ts` is what `agentConsole`
 * already imports from.
 */
export function preserveIterations<T extends { id: string; iterations: number }>(
  previous: T[],
  incoming: T[],
): T[] {
  if (previous.length === 0) return incoming;
  const prior = new Map(previous.map((agent) => [agent.id, agent.iterations]));
  return incoming.map((agent) => {
    const seen = prior.get(agent.id);
    return seen !== undefined && seen > agent.iterations
      ? { ...agent, iterations: seen }
      : agent;
  });
}
