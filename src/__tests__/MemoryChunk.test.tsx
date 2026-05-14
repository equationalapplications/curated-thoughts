import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { MemoryChunk } from '../components/review/MemoryChunk';

describe('MemoryChunk', () => {
  it('preserves emoji surrogate pairs when truncating', () => {
    const text = 'Hi 👍 there';
    const { container } = render(<MemoryChunk chunkText={text} maxLength={5} />);
    expect(container.textContent).toBe('Hi 👍…');
  });
});
