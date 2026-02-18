//
// distance.metal
// Metal shaders for batched distance computation
//
// Optimized for vector database queries: 1 query vector vs many database vectors
// Each GPU thread computes distance between query and one database vector
//

#include <metal_stdlib>
using namespace metal;

// ============================================================================
// BATCHED EUCLIDEAN DISTANCE (L2)
// ============================================================================
// Computes squared L2 distance: sum((a[i] - b[i])^2)
// Input: query[dim], vectors[n*dim], Output: distances[n]

kernel void euclidean_distance_batch(
    device const float* query [[buffer(0)]],      // Query vector [dim]
    device const float* vectors [[buffer(1)]],    // Database vectors [n * dim]
    device float* distances [[buffer(2)]],        // Output distances [n]
    constant uint& dim [[buffer(3)]],             // Vector dimension
    constant uint& n_vectors [[buffer(4)]],       // Number of vectors
    uint tid [[thread_position_in_grid]]          // Thread ID = vector index
) {
    if (tid >= n_vectors) return;

    // Pointer to this thread's database vector
    device const float* vec = vectors + tid * dim;

    // Compute squared L2 distance using SIMD accumulation
    float4 sum4 = float4(0.0f);
    uint i = 0;

    // Process 4 elements at a time (SIMD-4)
    uint dim4 = dim & ~3u;  // Round down to multiple of 4
    for (; i < dim4; i += 4) {
        float4 q = float4(query[i], query[i+1], query[i+2], query[i+3]);
        float4 v = float4(vec[i], vec[i+1], vec[i+2], vec[i+3]);
        float4 diff = q - v;
        sum4 += diff * diff;
    }

    // Sum the float4 components
    float sum = sum4.x + sum4.y + sum4.z + sum4.w;

    // Handle remaining elements
    for (; i < dim; i++) {
        float diff = query[i] - vec[i];
        sum += diff * diff;
    }

    distances[tid] = sum;
}

// ============================================================================
// BATCHED COSINE SIMILARITY
// ============================================================================
// Computes cosine similarity: dot(a, b) / (||a|| * ||b||)
// Input: query[dim], vectors[n*dim], Output: similarities[n]

kernel void cosine_similarity_batch(
    device const float* query [[buffer(0)]],      // Query vector [dim]
    device const float* vectors [[buffer(1)]],    // Database vectors [n * dim]
    device float* similarities [[buffer(2)]],     // Output similarities [n]
    constant uint& dim [[buffer(3)]],             // Vector dimension
    constant uint& n_vectors [[buffer(4)]],       // Number of vectors
    constant float& query_norm [[buffer(5)]],     // Pre-computed ||query||
    uint tid [[thread_position_in_grid]]
) {
    if (tid >= n_vectors) return;

    device const float* vec = vectors + tid * dim;

    // Compute dot product and vector norm simultaneously
    float4 dot4 = float4(0.0f);
    float4 norm4 = float4(0.0f);
    uint i = 0;

    uint dim4 = dim & ~3u;
    for (; i < dim4; i += 4) {
        float4 q = float4(query[i], query[i+1], query[i+2], query[i+3]);
        float4 v = float4(vec[i], vec[i+1], vec[i+2], vec[i+3]);
        dot4 += q * v;
        norm4 += v * v;
    }

    float dot_product = dot4.x + dot4.y + dot4.z + dot4.w;
    float vec_norm_sq = norm4.x + norm4.y + norm4.z + norm4.w;

    // Handle remaining elements
    for (; i < dim; i++) {
        dot_product += query[i] * vec[i];
        vec_norm_sq += vec[i] * vec[i];
    }

    // Compute cosine similarity
    float vec_norm = sqrt(vec_norm_sq);
    float denom = query_norm * vec_norm;

    // Handle zero vectors gracefully
    similarities[tid] = (denom > 1e-10f) ? (dot_product / denom) : 0.0f;
}

// ============================================================================
// BATCHED DOT PRODUCT (INNER PRODUCT)
// ============================================================================
// Computes dot product: sum(a[i] * b[i])
// For normalized vectors, this equals cosine similarity

kernel void dot_product_batch(
    device const float* query [[buffer(0)]],
    device const float* vectors [[buffer(1)]],
    device float* products [[buffer(2)]],
    constant uint& dim [[buffer(3)]],
    constant uint& n_vectors [[buffer(4)]],
    uint tid [[thread_position_in_grid]]
) {
    if (tid >= n_vectors) return;

    device const float* vec = vectors + tid * dim;

    float4 sum4 = float4(0.0f);
    uint i = 0;

    uint dim4 = dim & ~3u;
    for (; i < dim4; i += 4) {
        float4 q = float4(query[i], query[i+1], query[i+2], query[i+3]);
        float4 v = float4(vec[i], vec[i+1], vec[i+2], vec[i+3]);
        sum4 += q * v;
    }

    float sum = sum4.x + sum4.y + sum4.z + sum4.w;

    for (; i < dim; i++) {
        sum += query[i] * vec[i];
    }

    products[tid] = sum;
}

// ============================================================================
// BATCHED MANHATTAN DISTANCE (L1)
// ============================================================================
// Computes L1 distance: sum(|a[i] - b[i]|)

kernel void manhattan_distance_batch(
    device const float* query [[buffer(0)]],
    device const float* vectors [[buffer(1)]],
    device float* distances [[buffer(2)]],
    constant uint& dim [[buffer(3)]],
    constant uint& n_vectors [[buffer(4)]],
    uint tid [[thread_position_in_grid]]
) {
    if (tid >= n_vectors) return;

    device const float* vec = vectors + tid * dim;

    float4 sum4 = float4(0.0f);
    uint i = 0;

    uint dim4 = dim & ~3u;
    for (; i < dim4; i += 4) {
        float4 q = float4(query[i], query[i+1], query[i+2], query[i+3]);
        float4 v = float4(vec[i], vec[i+1], vec[i+2], vec[i+3]);
        sum4 += abs(q - v);
    }

    float sum = sum4.x + sum4.y + sum4.z + sum4.w;

    for (; i < dim; i++) {
        sum += abs(query[i] - vec[i]);
    }

    distances[tid] = sum;
}

// ============================================================================
// OPTIMIZED VERSIONS WITH SHARED MEMORY (for high dimensions)
// ============================================================================
// Uses threadgroup shared memory for query caching when dim > 256

kernel void euclidean_distance_batch_shared(
    device const float* query [[buffer(0)]],
    device const float* vectors [[buffer(1)]],
    device float* distances [[buffer(2)]],
    constant uint& dim [[buffer(3)]],
    constant uint& n_vectors [[buffer(4)]],
    uint tid [[thread_position_in_grid]],
    uint local_id [[thread_position_in_threadgroup]],
    uint group_size [[threads_per_threadgroup]],
    threadgroup float* shared_query [[threadgroup(0)]]
) {
    // Cooperatively load query into shared memory
    for (uint i = local_id; i < dim; i += group_size) {
        shared_query[i] = query[i];
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    if (tid >= n_vectors) return;

    device const float* vec = vectors + tid * dim;

    float4 sum4 = float4(0.0f);
    uint i = 0;

    uint dim4 = dim & ~3u;
    for (; i < dim4; i += 4) {
        float4 q = float4(shared_query[i], shared_query[i+1], shared_query[i+2], shared_query[i+3]);
        float4 v = float4(vec[i], vec[i+1], vec[i+2], vec[i+3]);
        float4 diff = q - v;
        sum4 += diff * diff;
    }

    float sum = sum4.x + sum4.y + sum4.z + sum4.w;

    for (; i < dim; i++) {
        float diff = shared_query[i] - vec[i];
        sum += diff * diff;
    }

    distances[tid] = sum;
}

// ============================================================================
// PAIRWISE DISTANCE MATRIX (P² Matrix for RAPTOR Engine)
// ============================================================================
// Computes all N×N pairwise distances in a single GPU call
// Each thread computes one distance D[row, col] = distance(vectors[row], vectors[col])
// Output is a flattened N×N matrix

kernel void pairwise_euclidean_matrix(
    device const float* vectors [[buffer(0)]],     // All vectors [n * dim]
    device float* distances [[buffer(1)]],         // Output matrix [n * n]
    constant uint& dim [[buffer(2)]],              // Vector dimension
    constant uint& n_vectors [[buffer(3)]],        // Number of vectors (N)
    uint2 tid [[thread_position_in_grid]]          // 2D thread position (row, col)
) {
    uint row = tid.y;
    uint col = tid.x;

    if (row >= n_vectors || col >= n_vectors) return;

    // Diagonal elements are 0
    if (row == col) {
        distances[row * n_vectors + col] = 0.0f;
        return;
    }

    // Pointers to the two vectors
    device const float* vec_a = vectors + row * dim;
    device const float* vec_b = vectors + col * dim;

    // Compute squared L2 distance using SIMD-4 accumulation
    float4 sum4 = float4(0.0f);
    uint i = 0;

    uint dim4 = dim & ~3u;
    for (; i < dim4; i += 4) {
        float4 a = float4(vec_a[i], vec_a[i+1], vec_a[i+2], vec_a[i+3]);
        float4 b = float4(vec_b[i], vec_b[i+1], vec_b[i+2], vec_b[i+3]);
        float4 diff = a - b;
        sum4 += diff * diff;
    }

    float sum = sum4.x + sum4.y + sum4.z + sum4.w;

    // Handle remaining elements
    for (; i < dim; i++) {
        float diff = vec_a[i] - vec_b[i];
        sum += diff * diff;
    }

    distances[row * n_vectors + col] = sum;
}

kernel void pairwise_cosine_matrix(
    device const float* vectors [[buffer(0)]],     // All vectors [n * dim]
    device const float* norms [[buffer(1)]],       // Pre-computed norms [n]
    device float* distances [[buffer(2)]],         // Output: 1 - similarity [n * n]
    constant uint& dim [[buffer(3)]],
    constant uint& n_vectors [[buffer(4)]],
    uint2 tid [[thread_position_in_grid]]
) {
    uint row = tid.y;
    uint col = tid.x;

    if (row >= n_vectors || col >= n_vectors) return;

    if (row == col) {
        distances[row * n_vectors + col] = 0.0f;
        return;
    }

    device const float* vec_a = vectors + row * dim;
    device const float* vec_b = vectors + col * dim;

    // Compute dot product
    float4 dot4 = float4(0.0f);
    uint i = 0;

    uint dim4 = dim & ~3u;
    for (; i < dim4; i += 4) {
        float4 a = float4(vec_a[i], vec_a[i+1], vec_a[i+2], vec_a[i+3]);
        float4 b = float4(vec_b[i], vec_b[i+1], vec_b[i+2], vec_b[i+3]);
        dot4 += a * b;
    }

    float dot_product = dot4.x + dot4.y + dot4.z + dot4.w;

    for (; i < dim; i++) {
        dot_product += vec_a[i] * vec_b[i];
    }

    // Cosine distance = 1 - similarity
    float denom = norms[row] * norms[col];
    float similarity = (denom > 1e-10f) ? (dot_product / denom) : 0.0f;
    distances[row * n_vectors + col] = 1.0f - similarity;
}

kernel void pairwise_dot_product_matrix(
    device const float* vectors [[buffer(0)]],
    device float* products [[buffer(1)]],          // Output [n * n]
    constant uint& dim [[buffer(2)]],
    constant uint& n_vectors [[buffer(3)]],
    uint2 tid [[thread_position_in_grid]]
) {
    uint row = tid.y;
    uint col = tid.x;

    if (row >= n_vectors || col >= n_vectors) return;

    device const float* vec_a = vectors + row * dim;
    device const float* vec_b = vectors + col * dim;

    float4 sum4 = float4(0.0f);
    uint i = 0;

    uint dim4 = dim & ~3u;
    for (; i < dim4; i += 4) {
        float4 a = float4(vec_a[i], vec_a[i+1], vec_a[i+2], vec_a[i+3]);
        float4 b = float4(vec_b[i], vec_b[i+1], vec_b[i+2], vec_b[i+3]);
        sum4 += a * b;
    }

    float sum = sum4.x + sum4.y + sum4.z + sum4.w;

    for (; i < dim; i++) {
        sum += vec_a[i] * vec_b[i];
    }

    // For dot product similarity, negate for distance ordering (higher = closer)
    products[row * n_vectors + col] = (row == col) ? 0.0f : -sum;
}

// ============================================================================
// TOP-K REDUCTION (per threadgroup)
// ============================================================================
// After computing distances, find top-k smallest within each threadgroup
// Final CPU merge is required for full top-k across all vectors

struct IndexedDistance {
    float distance;
    uint index;
};

// Simple insertion into sorted buffer of size k
inline void insert_if_smaller(
    thread IndexedDistance* topk,
    uint k,
    float distance,
    uint index
) {
    // Check if this distance is smaller than the largest in topk
    if (distance >= topk[k-1].distance) return;

    // Find insertion point
    uint pos = k - 1;
    while (pos > 0 && distance < topk[pos-1].distance) {
        topk[pos] = topk[pos-1];
        pos--;
    }
    topk[pos].distance = distance;
    topk[pos].index = index;
}

// Kernel that computes distances AND performs partial top-k reduction
kernel void euclidean_topk_partial(
    device const float* query [[buffer(0)]],
    device const float* vectors [[buffer(1)]],
    device float* out_distances [[buffer(2)]],    // [num_groups * k]
    device uint* out_indices [[buffer(3)]],       // [num_groups * k]
    constant uint& dim [[buffer(4)]],
    constant uint& n_vectors [[buffer(5)]],
    constant uint& k [[buffer(6)]],
    uint tid [[thread_position_in_grid]],
    uint local_id [[thread_position_in_threadgroup]],
    uint group_id [[threadgroup_position_in_grid]],
    uint group_size [[threads_per_threadgroup]],
    threadgroup IndexedDistance* shared_topk [[threadgroup(0)]]
) {
    // Each thread computes its distance
    float my_distance = INFINITY;
    uint my_index = tid;

    if (tid < n_vectors) {
        device const float* vec = vectors + tid * dim;

        float4 sum4 = float4(0.0f);
        uint i = 0;
        uint dim4 = dim & ~3u;

        for (; i < dim4; i += 4) {
            float4 q = float4(query[i], query[i+1], query[i+2], query[i+3]);
            float4 v = float4(vec[i], vec[i+1], vec[i+2], vec[i+3]);
            float4 diff = q - v;
            sum4 += diff * diff;
        }

        my_distance = sum4.x + sum4.y + sum4.z + sum4.w;

        for (; i < dim; i++) {
            float diff = query[i] - vec[i];
            my_distance += diff * diff;
        }
    }

    // Initialize shared topk (first k threads)
    if (local_id < k) {
        shared_topk[local_id].distance = INFINITY;
        shared_topk[local_id].index = 0xFFFFFFFF;
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // Each thread tries to insert into shared topk (atomic operations)
    // Using simple sequential insertion for correctness
    for (uint t = 0; t < group_size; t++) {
        if (local_id == t && my_distance < shared_topk[k-1].distance) {
            // Insert this thread's result
            uint pos = k - 1;
            while (pos > 0 && my_distance < shared_topk[pos-1].distance) {
                shared_topk[pos] = shared_topk[pos-1];
                pos--;
            }
            shared_topk[pos].distance = my_distance;
            shared_topk[pos].index = my_index;
        }
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }

    // First k threads write output
    if (local_id < k) {
        uint out_idx = group_id * k + local_id;
        out_distances[out_idx] = shared_topk[local_id].distance;
        out_indices[out_idx] = shared_topk[local_id].index;
    }
}
