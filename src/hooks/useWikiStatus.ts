import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

export interface WikiStatus {
  ingesting: boolean;
  librarian: boolean;
  heal: boolean;
  prune: boolean;
}

export function useWikiStatus(): WikiStatus {
  const [status, setStatus] = useState<WikiStatus>({
    ingesting: false,
    librarian: false,
    heal: false,
    prune: false,
  });

  useEffect(() => {
    const unsub = listen<WikiStatus>('wiki-status-change', (e) =>
      setStatus((prev) => ({ ...prev, ...e.payload })),
    );
    return () => {
      unsub.then((f) => f());
    };
  }, []);

  return status;
}
