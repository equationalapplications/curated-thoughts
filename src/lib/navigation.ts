import { useCallback, useMemo, useState } from "react";
import type { AppMode } from "../components/shell/ModeRail";

export interface NavTarget {
  mode: AppMode;
  entityId?: string;
  docPath?: string;
  proposalId?: string;
  taskId?: string;
  /**
   * Optional chunk anchor within the target document. When set together with
   * `docPath`, the library editor scrolls to and transiently highlights the
   * block identified by this id. v1: always undefined because `source_docs`
   * does not yet expose chunk ids.
   */
  chunkId?: string;
}

export interface NavigationState {
  current: NavTarget;
  canGoBack: boolean;
  canGoForward: boolean;
  navigate: (target: NavTarget) => void;
  goBack: () => void;
  goForward: () => void;
  reset: (target: NavTarget) => void;
}

interface Stacks {
  current: NavTarget;
  back: NavTarget[];
  forward: NavTarget[];
}

export function useNavigationState(
  initial: NavTarget = { mode: "brain" },
): NavigationState {
  const [state, setState] = useState<Stacks>({
    current: initial,
    back: [],
    forward: [],
  });

  const navigate = useCallback((target: NavTarget) => {
    setState((s) => ({
      current: target,
      back: [...s.back, s.current],
      forward: [],
    }));
  }, []);

  const goBack = useCallback(() => {
    setState((s) => {
      const prev = s.back.at(-1);
      if (!prev) return s;
      return {
        current: prev,
        back: s.back.slice(0, -1),
        forward: [s.current, ...s.forward],
      };
    });
  }, []);

  const goForward = useCallback(() => {
    setState((s) => {
      const next = s.forward[0];
      if (!next) return s;
      return {
        current: next,
        back: [...s.back, s.current],
        forward: s.forward.slice(1),
      };
    });
  }, []);

  const reset = useCallback((target: NavTarget) => {
    setState({
      current: target,
      back: [],
      forward: [],
    });
  }, []);

  return useMemo(
    () => ({
      current: state.current,
      canGoBack: state.back.length > 0,
      canGoForward: state.forward.length > 0,
      navigate,
      goBack,
      goForward,
      reset,
    }),
    [state, navigate, goBack, goForward, reset],
  );
}
