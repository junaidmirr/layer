use std::collections::HashMap;

/// High-performance deterministic subword/n-gram hashing vectorizer.
/// Produces normalized fixed-size dense embeddings (dimension D = 128)
/// capturing syntactic tokens, keywords, symbols, camelCase, snake_case identifiers.
#[derive(Debug, Clone)]
pub struct VectorEmbedder {
    dimension: usize,
}

impl Default for VectorEmbedder {
    fn default() -> Self {
        Self::new(128)
    }
}

impl VectorEmbedder {
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// Embed a piece of text (code or natural query) into a unit L2-normalized vector.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0.0f32; self.dimension];
        if text.trim().is_empty() {
            return vector;
        }

        let tokens = self.tokenize(text);
        if tokens.is_empty() {
            return vector;
        }

        // Term frequencies with subword hashing & position weight
        let mut tf: HashMap<usize, f32> = HashMap::new();
        for (i, token) in tokens.iter().enumerate() {
            let h = hash_token(token) as usize % self.dimension;
            let weight = 1.0 + 1.0 / (1.0 + (i as f32 * 0.05));
            *tf.entry(h).or_insert(0.0) += weight;

            // Also hash 3-grams of identifiers (character trigrams) for fuzzy/subword match
            if token.len() >= 3 {
                let bytes = token.as_bytes();
                for window in bytes.windows(3) {
                    let h3 = hash_bytes(window) as usize % self.dimension;
                    *tf.entry(h3).or_insert(0.0) += 0.4;
                }
            }
        }

        for (idx, weight) in tf {
            vector[idx] += weight;
        }

        // L2 normalize
        let norm_sq: f32 = vector.iter().map(|v| v * v).sum();
        if norm_sq > 0.0 {
            let norm = norm_sq.sqrt();
            for v in vector.iter_mut() {
                *v /= norm;
            }
        }

        vector
    }

    fn tokenize<'a>(&self, text: &'a str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();

        for ch in text.chars() {
            if ch.is_alphanumeric() || ch == '_' {
                current.push(ch.to_ascii_lowercase());
            } else {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }

        tokens
    }

    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        dot.clamp(-1.0, 1.0)
    }
}

fn hash_token(s: &str) -> u64 {
    hash_bytes(s.as_bytes())
}

fn hash_bytes(bytes: &[U64Bytes]) -> u64 {
    // FNV-1a 64-bit hash
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

type U64Bytes = u8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_similarity() {
        let embedder = VectorEmbedder::default();
        let v1 = embedder.embed("fn calculate_total_price(items: &[Item]) -> f64");
        let v2 = embedder.embed("calculate total price items");
        let v3 = embedder.embed("struct NetworkDatabaseConnectionConfig");

        let sim_close = VectorEmbedder::cosine_similarity(&v1, &v2);
        let sim_far = VectorEmbedder::cosine_similarity(&v1, &v3);

        assert!(sim_close > sim_far, "v1 and v2 should be closer than v1 and v3");
    }
}
