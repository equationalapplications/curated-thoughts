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
              <p className="related-chunk-text">
                {chunk.chunk_text.slice(0, 200)}
                {chunk.chunk_text.length > 200 ? "…" : ""}
              </p>
              <span className="related-chunk-score">
                {Math.round(chunk.score * 100)}% similar
              </span>
            </div>
          ))}
        </div>
      )}
    </aside>
  );
}
