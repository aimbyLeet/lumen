import { useState } from "react";
import { errorText } from "../../client/src/errors";

export type Run = (f: () => Promise<unknown>) => Promise<void>;

/** Asks the core for something: what went wrong stays to be shown, the state is read again either way. */
export function useRun(refresh: () => Promise<void>): { error: string; run: Run } {
  const [error, setError] = useState("");
  const run: Run = async (f) => {
    setError("");
    try {
      await f();
    } catch (e) {
      setError(errorText(e));
    }
    await refresh();
  };
  return { error, run };
}
