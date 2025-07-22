#!/usr/bin/env python3
import requests
import json

# Test the new DELETE endpoint
collection_id = 'test_collection_123'

# Create collection first
create_data = {
    'operation': 'create',
    'config': {
        'name': collection_id,
        'dimension': 128,
        'distance_metric': 'cosine'
    }
}

print('Creating collection...')
create_response = requests.post('http://localhost:5678/api/v1/collection', json=create_data)
print(f'Create response: {create_response.status_code}')

# Test DELETE
print(f'Testing DELETE /api/v1/collection/{collection_id}')
delete_response = requests.delete(f'http://localhost:5678/api/v1/collection/{collection_id}')
print(f'Delete response: {delete_response.status_code}')
if delete_response.status_code != 200:
    print(f'Delete error: {delete_response.text}')
else:
    print('Delete successful!')