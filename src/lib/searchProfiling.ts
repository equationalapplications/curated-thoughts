import { searchVault } from './tauri';

export interface SearchProfileResult {
  query: string;
  limit: number;
  rounds: number;
  latenciesMs: number[];
  meanMs: number;
}

export async function profileSearchLatency(
  query: string,
  limit = 10,
  rounds = 3,
): Promise<SearchProfileResult> {
  if (rounds < 1) throw new Error('rounds must be >= 1');
  if (limit < 1) throw new Error('limit must be >= 1');
  const latenciesMs: number[] = [];

  for (let i = 0; i < rounds; i += 1) {
    const start = performance.now();
    await searchVault(query, limit);
    const end = performance.now();
    latenciesMs.push(end - start);
  }

  const meanMs = latenciesMs.reduce((sum, ms) => sum + ms, 0) / latenciesMs.length;
  return {
    query,
    limit,
    rounds,
    latenciesMs,
    meanMs,
  };
}

export function logSearchProfile(result: SearchProfileResult) {
  console.group('Search Profiling');
  console.log('query:', result.query);
  console.log('limit:', result.limit);
  console.log('rounds:', result.rounds);
  console.log('latenciesMs:', result.latenciesMs.map((n) => `${n.toFixed(1)} ms`).join(', '));
  console.log('meanMs:', `${result.meanMs.toFixed(1)} ms`);
  console.groupEnd();
}
