/**
 * Example property test template (M1). Copy to .forall/scenarios/<requirement-id>.property.ts
 * and adjust imports. Requires fast-check in the project.
 */
import fc from "fast-check";
import { clamp } from "../../src/clamp.ts";

function parseFastCheckError(err: unknown): unknown {
  const msg = err instanceof Error ? err.message : String(err);
  const match = msg.match(/Counterexample:\s*(\[[^\]]+\])/);
  if (match) {
    try {
      const values = JSON.parse(match[1]) as unknown[];
      if (Array.isArray(values) && values.length === 3) {
        const [x, lo, hi] = values;
        return { x, lo, hi };
      }
      return values;
    } catch {
      // fall through
    }
  }
  if (err && typeof err === "object" && "counterexample" in err) {
    return (err as { counterexample: unknown }).counterexample;
  }
  return msg;
}

export default async function runPropertyTests(): Promise<{
  ok: boolean;
  counterexample?: unknown;
  seed?: number;
  examplesRun?: number;
}> {
  const seed = Number(process.env.FORALL_PBT_SEED ?? Date.now());
  const numRuns = Number(process.env.FORALL_PBT_EXAMPLES ?? 100);
  try {
    fc.assert(
      fc.property(fc.integer(), fc.integer(), fc.integer(), (x, lo, hi) => {
        fc.pre(lo <= hi);
        const r = clamp(x, lo, hi);
        return r >= lo && r <= hi;
      }),
      { numRuns, seed },
    );
    return { ok: true, seed, examplesRun: numRuns };
  } catch (e) {
    return {
      ok: false,
      seed,
      examplesRun: numRuns,
      counterexample: parseFastCheckError(e),
    };
  }
}
