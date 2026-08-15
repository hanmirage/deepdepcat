/** A persisted memory entry (backend: memory/store.rs Memory). */
export interface Memory {
  id: number;
  content: string;
  metadata: Record<string, unknown>;
  category: string;
  session_id: string | null;
  created_at: string;
  updated_at: string;
  access_count: number;
  last_accessed: string | null;
  decay_factor: number | null;
}

/** Result of a dream synthesis run (backend: memory/dream.rs DreamResult). */
export interface DreamResult {
  source_count: number;
  synthesized_count: number;
  summaries: string[];
}

/** A memory search result with relevance score (backend: memory/search.rs). */
export interface MemorySearchResult {
  memory: Memory;
  score: number;
  matched_terms: string[];
}
