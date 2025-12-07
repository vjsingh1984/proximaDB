/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */
package com.proximadb.embedded;

/**
 * Result of a vector similarity search.
 */
public class SearchResult {
    private final String id;
    private final float score;

    /**
     * Create a new search result.
     *
     * @param id Vector ID
     * @param score Similarity score (lower is more similar for distance metrics)
     */
    public SearchResult(String id, float score) {
        this.id = id;
        this.score = score;
    }

    /**
     * Get the vector ID.
     */
    public String getId() {
        return id;
    }

    /**
     * Get the similarity score.
     * Lower scores indicate more similar vectors for distance metrics.
     */
    public float getScore() {
        return score;
    }

    @Override
    public String toString() {
        return String.format("SearchResult{id='%s', score=%.4f}", id, score);
    }

    @Override
    public boolean equals(Object obj) {
        if (this == obj) return true;
        if (obj == null || getClass() != obj.getClass()) return false;
        SearchResult other = (SearchResult) obj;
        return id.equals(other.id) && Float.compare(score, other.score) == 0;
    }

    @Override
    public int hashCode() {
        int result = id.hashCode();
        result = 31 * result + Float.hashCode(score);
        return result;
    }
}
