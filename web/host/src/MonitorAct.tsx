import { useState } from "react";
import { errorText } from "../../client/src/errors";

/** A change sent to the service: `busy` is the label of the running one, `error` the last failure. */
export function useAct(refresh: () => Promise<void>) {
  const [error, setError] = useState("");
  const [busy, setBusy] = useState("");
  /** Runs `f`, shows the new state, then `done`; on failure keeps the error and shows the state as it is. */
  const act = async (what: string, f: () => Promise<unknown>, done: () => void) => {
    setError("");
    setBusy(what);
    try {
      await f();
      // The new state first: the old one must not flash in between.
      await refresh();
      done();
    } catch (e) {
      setError(errorText(e));
      await refresh();
    }
    setBusy("");
  };
  return { error, busy, act };
}

/** The failure of the last change, or the one the service reports; hidden while the service is off. */
export function ErrorBanner({ off, error, reported }: { off: boolean; error: string; reported: string }) {
  if (off || !(error || reported)) return null;
  return <p className="banner bad selectable">{error || errorText(reported)}</p>;
}
