import { HealthResponse, Collection } from '../types/api';

const BASE_URL = 'http://localhost:8080'; // Assuming ProximaDB server runs on this port

export const proximaDbApi = {
  async getHealth(): Promise<HealthResponse> {
    const response = await fetch(`${BASE_URL}/health`);
    if (!response.ok) {
      throw new Error(`HTTP error! status: ${response.status}`);
    }
    return response.json();
  },

  async listCollections(): Promise<Collection[]> {
    // For CollectionOperation, we need to send a POST request with the operation type
    const response = await fetch(`${BASE_URL}/collection_operation`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        operation: 'COLLECTION_LIST',
        // query_params and options can be added if needed for filtering/pagination
      }),
    });

    if (!response.ok) {
      throw new Error(`HTTP error! status: ${response.status}`);
    }
    const data = await response.json();
    // The API returns a CollectionResponse, which contains a 'collections' array
    return data.collections || [];
  },
};
