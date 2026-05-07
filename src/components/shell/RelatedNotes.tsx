import { useRelatedChunks } from "../../hooks/useRelatedChunks";

interface Props {
  selectedDoc: string | null;
}

export function RelatedNotes({ selectedDoc }: Props) {
  const chunks = useRelatedChunks(selectedDoc);

  return (
    <aside className="related-notes">
      <h3>Related Notes</h3>
      {chunks.length === 0 ? (
        <p className="placeholder">
          {selectedDoc ? "No related notes found" : "Select a document to see related notes"}
        </p>
      ) : (
        <div className="related-chunks">
          {chunks.map((chunk, i) => (
            <div key={i} className="related-chunk">
              <span className="related-chunk-path">
                {chunk.doc_path.split("/").at(-1)}:{chunk.start_line}
                {chunk.end_line !== chunk.start_line ? `–${chunk.end_line}` : ""}
              </span>
              <span className="related-chunk-meta" aria-label="chunk metadata">
                <span className="result-chip result-chip--strategy">{chunk.strategy}</span>
                <span className="result-chip result-chip--score">
                  {Math.round(chunk.score * 100)}% similar
                </span>
                {chunk.symbol_name ? (
                  <span className="result-chip result-chip--symbol" title={chunk.symbol_name}>
                    {chunk.symbol_name}
                  </span>
                ) : null}
              </span>
              <p className="related-chunk-text">
                {chunk.chunk_text.slice(0, 200)}
                {chunk.chunk_text.length > 200 ? "…" : ""}
              </p>
            </div>
          ))}
        </div>
      )}
    </aside>
  );
}
