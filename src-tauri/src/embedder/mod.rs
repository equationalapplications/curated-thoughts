use anyhow::Result;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

pub struct Embedder {
    model: TextEmbedding,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2)
                .with_show_download_progress(false),
        )?;
        Ok(Embedder { model })
    }

    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        Ok(self.model.embed(texts, None)?)
    }

    pub fn dimensions() -> usize {
        384
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm_a * norm_b)
    }

    #[test]
    fn test_embed_returns_correct_dimensions() {
        let embedder = Embedder::new().expect("model init");
        let vecs = embedder.embed(vec!["hello world".to_string()]).unwrap();
        assert_eq!(vecs.len(), 1);
        assert_eq!(vecs[0].len(), Embedder::dimensions());
    }

    #[test]
    fn test_embed_multiple_texts() {
        let embedder = Embedder::new().expect("model init");
        let vecs = embedder
            .embed(vec!["first sentence".to_string(), "second sentence".to_string()])
            .unwrap();
        assert_eq!(vecs.len(), 2);
    }

    #[test]
    fn test_similar_texts_have_high_cosine_similarity() {
        let embedder = Embedder::new().expect("model init");
        let vecs = embedder
            .embed(vec![
                "the cat sat on the mat".to_string(),
                "a cat was sitting on the mat".to_string(),
                "quantum physics and thermodynamics".to_string(),
            ])
            .unwrap();
        let sim_similar = cosine_similarity(&vecs[0], &vecs[1]);
        let sim_different = cosine_similarity(&vecs[0], &vecs[2]);
        assert!(sim_similar > sim_different, "similar texts should be closer");
    }
}
