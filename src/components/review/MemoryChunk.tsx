function emojiSafeSlice(text: string, maxLength: number): string {
  if (text.length <= maxLength) return text;
  let end = maxLength;

  // Mirror core-llm-wiki's safe chunking semantics by avoiding a split
  // within a UTF-16 surrogate pair boundary.
  if (end > 0 && end < text.length) {
    const prevCode = text.charCodeAt(end - 1);
    const nextCode = text.charCodeAt(end);
    if (prevCode >= 0xd800 && prevCode <= 0xdbff && nextCode >= 0xdc00 && nextCode <= 0xdfff) {
      end -= 1;
    }
  }

  return text.slice(0, end);
}

function getTierClass(tier?: string, docPath?: string) {
  if (tier === 'tier_fact') return 'memory-chunk--fact';
  if (tier === 'tier_wisdom') return 'memory-chunk--wisdom';
  if (tier === 'tier_working') return 'memory-chunk--working';
  // fallback: path heuristic for callers without entity_id
  if (!docPath) return 'memory-chunk--working';
  const normalized = docPath.replace(/\\/g, '/').toLowerCase();
  if (normalized.includes('/documents/')) return 'memory-chunk--fact';
  if (normalized.includes('/wiki/')) return 'memory-chunk--wisdom';
  return 'memory-chunk--working';
}

interface Props {
  chunkText: string;
  docPath?: string;
  tier?: string;
  maxLength?: number;
  className?: string;
}

export function MemoryChunk({
  chunkText,
  docPath,
  tier,
  maxLength = 120,
  className = '',
}: Props) {
  return (
    <span className={`memory-chunk ${getTierClass(tier, docPath)} ${className}`.trim()}>
      {emojiSafeSlice(chunkText, maxLength)}
      {chunkText.length > maxLength ? '…' : ''}
    </span>
  );
}
