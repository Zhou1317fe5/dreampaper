import { useCallback, useEffect, useRef, useState, type Dispatch, type RefObject } from 'react';
import { getWorkbenchProject, saveWorkbenchProject } from '../api';
import { isDirty, outgoingDoc, type Action, type EditorState } from './state';

export const AUTOSAVE_DELAY_MS = 800;
export type SaveStatus = 'clean' | 'dirty' | 'saving' | 'saved' | 'failed';
type Phase = 'idle' | 'saving' | 'saved' | 'failed';

export interface SaveController {
  status: SaveStatus;
  error: string | null;
  flush: () => Promise<boolean>;
  discard: () => Promise<void>;
}

export class Saver {
  private inflight: Promise<boolean> | null = null;

  constructor(
    private readonly current: () => EditorState,
    private readonly dispatch: Dispatch<Action>,
    private readonly report: (phase: Phase, error: string | null) => void,
    private readonly persist = saveWorkbenchProject,
    private readonly load = getWorkbenchProject
  ) {}

  flush(): Promise<boolean> {
    if (this.inflight) return this.inflight;
    const run = async () => {
      try {
        while (isDirty(this.current())) {
          const state = this.current();
          if (state.transientBase) return false;
          this.report('saving', null);
          const detail = await this.persist(state.projectId, state.revision, outgoingDoc(state));
          this.dispatch({ type: 'saved', base: state.doc, document: detail.document });
        }
        this.report('saved', null);
        return true;
      } catch (error) {
        this.report('failed', error instanceof Error ? error.message : String(error));
        return false;
      }
    };
    this.inflight = run().finally(() => { this.inflight = null; });
    return this.inflight;
  }

  async discard(): Promise<void> {
    await this.inflight;
    const detail = await this.load(this.current().projectId);
    this.dispatch({ type: 'load', detail });
    this.report('idle', null);
  }
}

export function useAutosave(current: RefObject<EditorState>, dispatch: Dispatch<Action>): SaveController {
  const [phase, setPhase] = useState<Phase>('idle');
  const [error, setError] = useState<string | null>(null);
  const saver = useRef<Saver | null>(null);
  if (!saver.current) {
    saver.current = new Saver(() => current.current, dispatch, (next, failure) => {
      setPhase(next);
      setError(failure);
    });
  }
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancelTimer = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  }, []);
  const flush = useCallback(() => {
    cancelTimer();
    return saver.current!.flush();
  }, [cancelTimer]);
  const discard = useCallback(() => {
    cancelTimer();
    return saver.current!.discard();
  }, [cancelTimer]);

  const state = current.current;
  useEffect(() => {
    if (isDirty(state) && !state.transientBase) {
      timer.current = setTimeout(() => { void flush(); }, AUTOSAVE_DELAY_MS);
    }
    return cancelTimer;
  }, [state.doc, state.savedDoc, state.transientBase, flush, cancelTimer]);

  const status: SaveStatus = phase === 'saving' ? 'saving' : phase === 'failed' ? 'failed' : isDirty(state) ? 'dirty' : phase === 'saved' ? 'saved' : 'clean';
  return { status, error, flush, discard };
}
