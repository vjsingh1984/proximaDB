/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */
package com.proximadb.embedded;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Configuration for a storage disk/directory in multi-disk setups.
 *
 * <p>Example usage:
 * <pre>{@code
 * List<DiskConfig> disks = Arrays.asList(
 *     new DiskConfig("/nvme/data", 2),   // Fast SSD with weight 2
 *     new DiskConfig("/hdd/data", 1)     // Slower HDD with weight 1
 * );
 * }</pre>
 */
public class DiskConfig {
    private final String path;
    private final int weight;
    private final List<String> tags;

    /**
     * Create a disk configuration with default weight of 1.
     *
     * @param path Path to the storage directory
     */
    public DiskConfig(String path) {
        this(path, 1);
    }

    /**
     * Create a disk configuration with a specific weight.
     *
     * @param path Path to the storage directory
     * @param weight Weight for data distribution (higher = more data)
     */
    public DiskConfig(String path, int weight) {
        this(path, weight, new ArrayList<>());
    }

    /**
     * Create a disk configuration with weight and tags.
     *
     * @param path Path to the storage directory
     * @param weight Weight for data distribution
     * @param tags Tags for storage tier identification (e.g., "hot", "cold")
     */
    public DiskConfig(String path, int weight, List<String> tags) {
        this.path = path;
        this.weight = weight;
        this.tags = new ArrayList<>(tags);
    }

    /**
     * Get the storage path.
     */
    public String getPath() {
        return path;
    }

    /**
     * Get the weight for data distribution.
     */
    public int getWeight() {
        return weight;
    }

    /**
     * Get the tags for this storage location.
     */
    public List<String> getTags() {
        return Collections.unmodifiableList(tags);
    }

    @Override
    public String toString() {
        return String.format("DiskConfig{path='%s', weight=%d, tags=%s}", path, weight, tags);
    }
}
