# ProximaDB Java Client Examples

Complete examples for using ProximaDB with Java clients for both REST and gRPC APIs.

## Dependencies

### Maven

```xml
<dependencies>
    <!-- Official ProximaDB Java Client -->
    <dependency>
        <groupId>ai.proximadb</groupId>
        <artifactId>proximadb-java-client</artifactId>
        <version>0.1.0</version>
    </dependency>
    
    <!-- For manual REST integration -->
    <dependency>
        <groupId>com.squareup.okhttp3</groupId>
        <artifactId>okhttp</artifactId>
        <version>4.12.0</version>
    </dependency>
    <dependency>
        <groupId>com.fasterxml.jackson.core</groupId>
        <artifactId>jackson-databind</artifactId>
        <version>2.16.0</version>
    </dependency>
    
    <!-- For gRPC integration -->
    <dependency>
        <groupId>io.grpc</groupId>
        <artifactId>grpc-netty-shaded</artifactId>
        <version>1.60.0</version>
    </dependency>
    <dependency>
        <groupId>io.grpc</groupId>
        <artifactId>grpc-protobuf</artifactId>
        <version>1.60.0</version>
    </dependency>
    <dependency>
        <groupId>io.grpc</groupId>
        <artifactId>grpc-stub</artifactId>
        <version>1.60.0</version>
    </dependency>
</dependencies>
```

### Gradle

```gradle
dependencies {
    // Official ProximaDB Java Client
    implementation 'ai.proximadb:proximadb-java-client:0.1.0'
    
    // For manual REST integration
    implementation 'com.squareup.okhttp3:okhttp:4.12.0'
    implementation 'com.fasterxml.jackson.core:jackson-databind:2.16.0'
    
    // For gRPC integration
    implementation 'io.grpc:grpc-netty-shaded:1.60.0'
    implementation 'io.grpc:grpc-protobuf:1.60.0'
    implementation 'io.grpc:grpc-stub:1.60.0'
}
```

## REST API Examples

### Basic Setup

```java
import okhttp3.*;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.JsonNode;
import java.io.IOException;
import java.util.*;
import java.util.concurrent.TimeUnit;

public class ProximaDBRestClient {
    private static final MediaType JSON = MediaType.get("application/json; charset=utf-8");
    
    private final OkHttpClient client;
    private final ObjectMapper objectMapper;
    private final String baseUrl;
    private final String apiKey;
    
    public ProximaDBRestClient(String baseUrl, String apiKey) {
        this.baseUrl = baseUrl.replaceAll("/$", "");
        this.apiKey = apiKey;
        this.objectMapper = new ObjectMapper();
        this.client = new OkHttpClient.Builder()
                .connectTimeout(30, TimeUnit.SECONDS)
                .readTimeout(60, TimeUnit.SECONDS)
                .build();
    }
    
    public JsonNode request(String method, String endpoint, Object data) throws IOException {
        Request.Builder requestBuilder = new Request.Builder()
                .url(baseUrl + endpoint);
        
        if (apiKey != null) {
            requestBuilder.addHeader("Authorization", "Bearer " + apiKey);
        }
        
        if (data != null) {
            String json = objectMapper.writeValueAsString(data);
            RequestBody body = RequestBody.create(json, JSON);
            requestBuilder.method(method, body);
        } else {
            requestBuilder.method(method, null);
        }
        
        try (Response response = client.newCall(requestBuilder.build()).execute()) {
            String responseBody = response.body().string();
            
            if (!response.isSuccessful()) {
                JsonNode errorNode = objectMapper.readTree(responseBody);
                throw new ProximaDBException(
                    errorNode.path("error").path("code").asText("UNKNOWN_ERROR"),
                    errorNode.path("error").path("message").asText(responseBody)
                );
            }
            
            return objectMapper.readTree(responseBody);
        }
    }
}

class ProximaDBException extends RuntimeException {
    private final String code;
    
    public ProximaDBException(String code, String message) {
        super(code + ": " + message);
        this.code = code;
    }
    
    public String getCode() {
        return code;
    }
}

// Initialize client
ProximaDBRestClient client = new ProximaDBRestClient(
    "http://localhost:5678", 
    "pk_live_1234567890abcdef"
);
```

### Collection Management

```java
import java.util.*;

public class CollectionExamples {
    private final ProximaDBRestClient client;
    
    public CollectionExamples(ProximaDBRestClient client) {
        this.client = client;
    }
    
    // Create a collection
    public JsonNode createCollectionExample() throws IOException {
        Map<String, Object> collectionData = new HashMap<>();
        collectionData.put("collection_id", "java_embeddings");
        collectionData.put("dimension", 384);
        collectionData.put("distance_metric", "cosine");
        collectionData.put("description", "Java document embeddings");
        
        try {
            JsonNode result = client.request("POST", "/collections", collectionData);
            System.out.println("Collection created: " + result.path("collection_id").asText());
            return result;
        } catch (ProximaDBException e) {
            if ("COLLECTION_ALREADY_EXISTS".equals(e.getCode())) {
                System.out.println("Collection already exists, continuing...");
                Map<String, String> existingCollection = new HashMap<>();
                existingCollection.put("collection_id", "java_embeddings");
                return new ObjectMapper().valueToTree(existingCollection);
            }
            throw e;
        }
    }
    
    // List all collections
    public JsonNode listCollectionsExample() throws IOException {
        JsonNode collections = client.request("GET", "/collections", null);
        JsonNode collectionList = collections.path("collections");
        
        System.out.println("Found " + collectionList.size() + " collections:");
        for (JsonNode collection : collectionList) {
            System.out.printf("- %s: %d vectors%n", 
                collection.path("id").asText(),
                collection.path("vector_count").asInt()
            );
        }
        
        return collections;
    }
    
    // Get collection details
    public JsonNode getCollectionExample(String collectionId) throws IOException {
        JsonNode collection = client.request("GET", "/collections/" + collectionId, null);
        System.out.println("Collection " + collection.path("id").asText() + ":");
        System.out.println("  Dimension: " + collection.path("dimension").asInt());
        System.out.println("  Vectors: " + collection.path("vector_count").asInt());
        System.out.println("  Size: " + collection.path("total_size_bytes").asLong() + " bytes");
        return collection;
    }
    
    // Delete a collection
    public JsonNode deleteCollectionExample(String collectionId) throws IOException {
        JsonNode result = client.request("DELETE", "/collections/" + collectionId, null);
        System.out.println("Collection deleted: " + result.path("message").asText());
        return result;
    }
}
```

### Vector Operations

```java
import java.util.*;
import java.util.stream.DoubleStream;

public class VectorExamples {
    private final ProximaDBRestClient client;
    private final Random random = new Random();
    
    public VectorExamples(ProximaDBRestClient client) {
        this.client = client;
    }
    
    // Generate random vector for testing
    private List<Double> generateRandomVector(int dimension) {
        return DoubleStream.generate(random::nextDouble)
                .limit(dimension)
                .boxed()
                .collect(ArrayList::new, ArrayList::add, ArrayList::addAll);
    }
    
    // Insert a single vector
    public JsonNode insertVectorExample() throws IOException {
        Map<String, Object> vectorData = new HashMap<>();
        vectorData.put("id", UUID.randomUUID().toString());
        vectorData.put("vector", generateRandomVector(384));
        
        Map<String, Object> metadata = new HashMap<>();
        metadata.put("document_id", "java_doc_001");
        metadata.put("title", "Java Example Document");
        metadata.put("category", "programming");
        metadata.put("tags", Arrays.asList("java", "spring", "api"));
        vectorData.put("metadata", metadata);
        
        JsonNode result = client.request("POST", "/collections/java_embeddings/vectors", vectorData);
        System.out.println("Vector inserted: " + result.path("vector_id").asText());
        return result;
    }
    
    // Get a vector by ID
    public JsonNode getVectorExample(String collectionId, String vectorId) throws IOException {
        JsonNode vector = client.request("GET", 
            String.format("/collections/%s/vectors/%s", collectionId, vectorId), null);
        
        System.out.println("Vector " + vector.path("id").asText() + ":");
        System.out.println("  Dimension: " + vector.path("vector").size());
        System.out.println("  Metadata: " + vector.path("metadata"));
        return vector;
    }
    
    // Search for similar vectors
    public JsonNode searchVectorsExample() throws IOException {
        List<Double> queryVector = generateRandomVector(384);
        
        Map<String, Object> searchData = new HashMap<>();
        searchData.put("vector", queryVector);
        searchData.put("k", 10);
        
        Map<String, String> filter = new HashMap<>();
        filter.put("category", "programming");
        searchData.put("filter", filter);
        searchData.put("threshold", 0.7);
        
        JsonNode results = client.request("POST", "/collections/java_embeddings/search", searchData);
        
        System.out.println("Found " + results.path("total_count").asInt() + " similar vectors:");
        for (JsonNode result : results.path("results")) {
            System.out.printf("- %s: score=%.3f%n", 
                result.path("id").asText(),
                result.path("score").asDouble()
            );
            if (result.has("metadata")) {
                System.out.println("  Title: " + 
                    result.path("metadata").path("title").asText("N/A"));
            }
        }
        
        return results;
    }
    
    // Delete a vector
    public JsonNode deleteVectorExample(String collectionId, String vectorId) throws IOException {
        JsonNode result = client.request("DELETE", 
            String.format("/collections/%s/vectors/%s", collectionId, vectorId), null);
        System.out.println("Vector deleted: " + result.path("message").asText());
        return result;
    }
}
```

### Batch Operations

```java
import java.util.*;
import java.util.stream.IntStream;

public class BatchExamples {
    private final ProximaDBRestClient client;
    private final Random random = new Random();
    
    public BatchExamples(ProximaDBRestClient client) {
        this.client = client;
    }
    
    private List<Double> generateRandomVector(int dimension) {
        return DoubleStream.generate(random::nextDouble)
                .limit(dimension)
                .boxed()
                .collect(ArrayList::new, ArrayList::add, ArrayList::addAll);
    }
    
    // Batch insert vectors
    public JsonNode batchInsertExample() throws IOException {
        List<Map<String, Object>> vectors = new ArrayList<>();
        
        for (int i = 0; i < 100; i++) {
            Map<String, Object> vectorData = new HashMap<>();
            vectorData.put("id", UUID.randomUUID().toString());
            vectorData.put("vector", generateRandomVector(384));
            
            Map<String, Object> metadata = new HashMap<>();
            metadata.put("document_id", String.format("java_doc_%03d", i));
            metadata.put("batch", "java_example_batch");
            metadata.put("index", i);
            vectorData.put("metadata", metadata);
            
            vectors.add(vectorData);
        }
        
        Map<String, Object> batchData = new HashMap<>();
        batchData.put("vectors", vectors);
        
        JsonNode result = client.request("POST", 
            "/collections/java_embeddings/vectors/batch", batchData);
        
        System.out.println("Batch inserted " + result.path("inserted_count").asInt() + " vectors");
        if (result.has("errors") && result.path("errors").size() > 0) {
            System.out.println("Errors: " + result.path("errors"));
        }
        
        return result;
    }
    
    // Batch search
    public JsonNode batchSearchExample() throws IOException {
        List<Map<String, Object>> searches = new ArrayList<>();
        
        for (int i = 0; i < 5; i++) {
            Map<String, Object> search = new HashMap<>();
            search.put("collection_id", "java_embeddings");
            search.put("vector", generateRandomVector(384));
            search.put("k", 5);
            
            Map<String, String> filter = new HashMap<>();
            filter.put("batch", "java_example_batch");
            search.put("filter", filter);
            
            searches.add(search);
        }
        
        Map<String, Object> batchData = new HashMap<>();
        batchData.put("searches", searches);
        
        JsonNode results = client.request("POST", "/batch/search", batchData);
        
        System.out.println("Performed " + results.path("results").size() + " searches:");
        JsonNode resultList = results.path("results");
        for (int i = 0; i < resultList.size(); i++) {
            JsonNode searchResult = resultList.get(i);
            System.out.printf("  Search %d: %d results%n", 
                i, searchResult.path("total_count").asInt());
        }
        
        return results;
    }
}
```

### Complete Example Application

```java
/**
 * ProximaDB REST API Example Application
 * Demonstrates document similarity search using embeddings.
 */

import java.io.IOException;
import java.util.*;
import java.util.stream.IntStream;

public class DocumentSearchApp {
    private final ProximaDBRestClient client;
    private final String collectionId = "java_document_search";
    private final Random random = new Random();
    
    public DocumentSearchApp(String apiKey) {
        this.client = new ProximaDBRestClient("http://localhost:5678", apiKey);
    }
    
    public void setup() throws IOException {
        // Initialize the collection for document search
        try {
            Map<String, Object> collectionData = new HashMap<>();
            collectionData.put("collection_id", collectionId);
            collectionData.put("dimension", 384);
            collectionData.put("distance_metric", "cosine");
            collectionData.put("description", "Java document similarity search");
            
            client.request("POST", "/collections", collectionData);
            System.out.println("Collection created successfully");
        } catch (ProximaDBException e) {
            if ("COLLECTION_ALREADY_EXISTS".equals(e.getCode())) {
                System.out.println("Collection already exists");
            } else {
                throw e;
            }
        }
    }
    
    // Simulate text embeddings (in practice, use a real embedding model)
    private List<Double> generateSimulatedEmbedding(String text) {
        // Simple hash-based simulation for demo purposes
        Random textRandom = new Random(text.hashCode());
        return IntStream.range(0, 384)
                .mapToDouble(i -> textRandom.nextGaussian())
                .boxed()
                .collect(ArrayList::new, ArrayList::add, ArrayList::addAll);
    }
    
    public JsonNode addDocuments(List<Document> documents) throws IOException {
        List<Map<String, Object>> vectors = new ArrayList<>();
        
        for (Document doc : documents) {
            // Generate embedding (simulate with hash-based random)
            List<Double> embedding = generateSimulatedEmbedding(doc.content);
            
            Map<String, Object> vectorData = new HashMap<>();
            vectorData.put("vector", embedding);
            
            Map<String, Object> metadata = new HashMap<>();
            metadata.put("title", doc.title);
            metadata.put("content", doc.content.length() > 200 ? 
                doc.content.substring(0, 200) : doc.content); // Truncate for storage
            metadata.put("author", doc.author != null ? doc.author : "Unknown");
            metadata.put("category", doc.category != null ? doc.category : "general");
            vectorData.put("metadata", metadata);
            
            vectors.add(vectorData);
        }
        
        // Batch insert
        Map<String, Object> batchData = new HashMap<>();
        batchData.put("vectors", vectors);
        
        JsonNode result = client.request("POST", 
            String.format("/collections/%s/vectors/batch", collectionId), batchData);
        
        System.out.println("Added " + result.path("inserted_count").asInt() + 
            " documents to search index");
        return result;
    }
    
    public List<SearchResult> searchDocuments(String query, int k, String categoryFilter) 
            throws IOException {
        // Generate query embedding
        List<Double> queryEmbedding = generateSimulatedEmbedding(query);
        
        Map<String, Object> searchData = new HashMap<>();
        searchData.put("vector", queryEmbedding);
        searchData.put("k", k);
        searchData.put("threshold", 0.3);
        
        // Add category filter if specified
        if (categoryFilter != null) {
            Map<String, String> filter = new HashMap<>();
            filter.put("category", categoryFilter);
            searchData.put("filter", filter);
        }
        
        JsonNode results = client.request("POST", 
            String.format("/collections/%s/search", collectionId), searchData);
        
        List<SearchResult> documents = new ArrayList<>();
        for (JsonNode result : results.path("results")) {
            SearchResult doc = new SearchResult();
            doc.id = result.path("id").asText();
            doc.score = result.path("score").asDouble();
            doc.title = result.path("metadata").path("title").asText();
            doc.author = result.path("metadata").path("author").asText();
            doc.content = result.path("metadata").path("content").asText();
            documents.add(doc);
        }
        
        return documents;
    }
    
    public static void main(String[] args) throws IOException {
        // Initialize the application
        DocumentSearchApp app = new DocumentSearchApp("pk_test_example123");
        app.setup();
        
        // Sample documents
        List<Document> documents = Arrays.asList(
            new Document(
                "Introduction to Machine Learning",
                "Machine learning is a subset of artificial intelligence that focuses on algorithms that can learn from data.",
                "Dr. Smith",
                "education"
            ),
            new Document(
                "Deep Learning Fundamentals", 
                "Deep learning uses neural networks with multiple layers to model complex patterns in data.",
                "Prof. Johnson",
                "education"
            ),
            new Document(
                "Java Spring Framework Guide",
                "Spring Framework is a comprehensive programming and configuration model for Java applications.",
                "Java Expert",
                "programming"
            ),
            new Document(
                "Microservices Architecture",
                "Microservices architecture is an approach to building applications as suites of independently deployable services.",
                "Architecture Guru",
                "programming"
            )
        );
        
        // Add documents to the index
        app.addDocuments(documents);
        
        // Search for documents
        System.out.println("\n--- Search Results ---");
        String[] queries = {
            "artificial intelligence algorithms",
            "neural networks and deep learning", 
            "Java programming frameworks",
            "distributed system architecture"
        };
        
        for (String query : queries) {
            System.out.println("\nQuery: '" + query + "'");
            List<SearchResult> results = app.searchDocuments(query, 3, null);
            
            for (int i = 0; i < results.size(); i++) {
                SearchResult doc = results.get(i);
                System.out.printf("%d. %s (score: %.3f)%n", 
                    i + 1, doc.title, doc.score);
                System.out.printf("   Author: %s%n", doc.author);
                System.out.printf("   Content: %s...%n", doc.content);
            }
        }
    }
}

// Data classes
class Document {
    public final String title;
    public final String content;
    public final String author;
    public final String category;
    
    public Document(String title, String content, String author, String category) {
        this.title = title;
        this.content = content;
        this.author = author;
        this.category = category;
    }
}

class SearchResult {
    public String id;
    public double score;
    public String title;
    public String author;
    public String content;
}
```

## gRPC API Examples

### Basic Setup

```java
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Metadata;
import io.grpc.stub.MetadataUtils;
import ai.proximadb.proto.VectorDBGrpc;
import ai.proximadb.proto.VectordbProto.*;
import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import java.util.concurrent.TimeUnit;

public class ProximaDBGrpcClient {
    private final ManagedChannel channel;
    private final VectorDBGrpc.VectorDBBlockingStub stub;
    private final String apiKey;
    
    public ProximaDBGrpcClient(String host, int port, String apiKey) {
        this.apiKey = apiKey;
        this.channel = ManagedChannelBuilder
                .forAddress(host, port)
                .usePlaintext()
                .build();
        
        VectorDBGrpc.VectorDBBlockingStub baseStub = VectorDBGrpc.newBlockingStub(channel);
        
        if (apiKey != null) {
            Metadata metadata = new Metadata();
            metadata.put(Metadata.Key.of("authorization", Metadata.ASCII_STRING_MARSHALLER), 
                "Bearer " + apiKey);
            this.stub = baseStub.withInterceptors(MetadataUtils.newAttachHeadersInterceptor(metadata));
        } else {
            this.stub = baseStub;
        }
    }
    
    public void shutdown() throws InterruptedException {
        channel.shutdown().awaitTermination(5, TimeUnit.SECONDS);
    }
    
    public VectorDBGrpc.VectorDBBlockingStub getStub() {
        return stub;
    }
}

// Initialize client
ProximaDBGrpcClient grpcClient = new ProximaDBGrpcClient(
    "localhost", 9090, "pk_live_1234567890abcdef"
);
```

### Collection Management with gRPC

```java
public class GrpcCollectionExamples {
    private final ProximaDBGrpcClient client;
    
    public GrpcCollectionExamples(ProximaDBGrpcClient client) {
        this.client = client;
    }
    
    public CreateCollectionResponse createCollectionGrpc() {
        CreateCollectionRequest request = CreateCollectionRequest.newBuilder()
                .setCollectionId("grpc_java_embeddings")
                .setName("gRPC Java Document Embeddings")
                .setDimension(384)
                .setSchemaType(SchemaType.SCHEMA_TYPE_DOCUMENT)
                .build();
        
        CreateCollectionResponse response = client.getStub().createCollection(request);
        System.out.println("Collection created: " + response.getSuccess());
        return response;
    }
    
    public ListCollectionsResponse listCollectionsGrpc() {
        ListCollectionsRequest request = ListCollectionsRequest.newBuilder().build();
        ListCollectionsResponse response = client.getStub().listCollections(request);
        
        System.out.println("Found " + response.getCollectionsCount() + " collections:");
        for (Collection collection : response.getCollectionsList()) {
            System.out.printf("- %s: %d vectors%n", 
                collection.getId(), collection.getVectorCount());
        }
        
        return response;
    }
    
    public DeleteCollectionResponse deleteCollectionGrpc(String collectionId) {
        DeleteCollectionRequest request = DeleteCollectionRequest.newBuilder()
                .setCollectionId(collectionId)
                .build();
        
        DeleteCollectionResponse response = client.getStub().deleteCollection(request);
        System.out.println("Collection deleted: " + response.getSuccess());
        return response;
    }
}
```

### Vector Operations with gRPC

```java
import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import java.util.*;

public class GrpcVectorExamples {
    private final ProximaDBGrpcClient client;
    private final Random random = new Random();
    
    public GrpcVectorExamples(ProximaDBGrpcClient client) {
        this.client = client;
    }
    
    private Struct createMetadataStruct(Map<String, Object> metadataMap) {
        Struct.Builder structBuilder = Struct.newBuilder();
        
        for (Map.Entry<String, Object> entry : metadataMap.entrySet()) {
            Value.Builder valueBuilder = Value.newBuilder();
            Object value = entry.getValue();
            
            if (value instanceof String) {
                valueBuilder.setStringValue((String) value);
            } else if (value instanceof Number) {
                valueBuilder.setNumberValue(((Number) value).doubleValue());
            } else if (value instanceof Boolean) {
                valueBuilder.setBoolValue((Boolean) value);
            } else if (value instanceof List) {
                Value.Builder listBuilder = Value.newBuilder();
                ListValue.Builder listValueBuilder = ListValue.newBuilder();
                for (Object item : (List<?>) value) {
                    listValueBuilder.addValues(Value.newBuilder().setStringValue(item.toString()));
                }
                valueBuilder.setListValue(listValueBuilder);
            }
            
            structBuilder.putFields(entry.getKey(), valueBuilder.build());
        }
        
        return structBuilder.build();
    }
    
    private List<Float> generateRandomVector(int dimension) {
        List<Float> vector = new ArrayList<>();
        for (int i = 0; i < dimension; i++) {
            vector.add(random.nextFloat());
        }
        return vector;
    }
    
    public InsertResponse insertVectorGrpc() {
        // Create metadata
        Map<String, Object> metadataMap = new HashMap<>();
        metadataMap.put("document_id", "grpc_java_doc_001");
        metadataMap.put("title", "gRPC Java Example Document");
        metadataMap.put("category", "technical");
        metadataMap.put("tags", Arrays.asList("grpc", "java", "api"));
        
        Struct metadata = createMetadataStruct(metadataMap);
        
        // Create vector record
        VectorRecord record = VectorRecord.newBuilder()
                .setId(UUID.randomUUID().toString())
                .setCollectionId("grpc_java_embeddings")
                .addAllVector(generateRandomVector(384))
                .setMetadata(metadata)
                .build();
        
        InsertRequest request = InsertRequest.newBuilder()
                .setCollectionId("grpc_java_embeddings")
                .setRecord(record)
                .build();
        
        InsertResponse response = client.getStub().insert(request);
        System.out.println("Vector inserted: " + response.getVectorId());
        return response;
    }
    
    public SearchResponse searchVectorsGrpc() {
        List<Float> queryVector = generateRandomVector(384);
        
        // Create filter
        Map<String, Object> filterMap = new HashMap<>();
        filterMap.put("category", "technical");
        Struct filters = createMetadataStruct(filterMap);
        
        SearchRequest request = SearchRequest.newBuilder()
                .setCollectionId("grpc_java_embeddings")
                .addAllVector(queryVector)
                .setK(10)
                .setFilters(filters)
                .setThreshold(0.7f)
                .build();
        
        SearchResponse response = client.getStub().search(request);
        
        System.out.println("Found " + response.getTotalCount() + " similar vectors:");
        for (SearchResult result : response.getResultsList()) {
            System.out.printf("- %s: score=%.3f%n", result.getId(), result.getScore());
            if (result.hasMetadata()) {
                Value titleValue = result.getMetadata().getFieldsMap().get("title");
                if (titleValue != null) {
                    System.out.println("  Title: " + titleValue.getStringValue());
                }
            }
        }
        
        return response;
    }
    
    public BatchInsertResponse batchInsertGrpc() {
        List<VectorRecord> records = new ArrayList<>();
        
        for (int i = 0; i < 50; i++) {
            Map<String, Object> metadataMap = new HashMap<>();
            metadataMap.put("document_id", String.format("grpc_java_batch_%03d", i));
            metadataMap.put("index", i);
            metadataMap.put("batch", "grpc_java_example");
            
            Struct metadata = createMetadataStruct(metadataMap);
            
            VectorRecord record = VectorRecord.newBuilder()
                    .setCollectionId("grpc_java_embeddings")
                    .addAllVector(generateRandomVector(384))
                    .setMetadata(metadata)
                    .build();
            
            records.add(record);
        }
        
        BatchInsertRequest request = BatchInsertRequest.newBuilder()
                .setCollectionId("grpc_java_embeddings")
                .addAllRecords(records)
                .build();
        
        BatchInsertResponse response = client.getStub().batchInsert(request);
        System.out.println("Batch inserted " + response.getInsertedCount() + " vectors");
        
        if (response.getErrorsCount() > 0) {
            System.out.println("Errors: " + response.getErrorsList());
        }
        
        return response;
    }
}
```

### Health and Monitoring

```java
public class GrpcHealthExamples {
    private final ProximaDBGrpcClient client;
    
    public GrpcHealthExamples(ProximaDBGrpcClient client) {
        this.client = client;
    }
    
    public HealthResponse healthCheckGrpc() {
        HealthRequest request = HealthRequest.newBuilder().build();
        HealthResponse response = client.getStub().health(request);
        
        System.out.println("Health status: " + response.getStatus());
        System.out.println("Timestamp: " + response.getTimestamp());
        return response;
    }
    
    public StatusResponse getStatusGrpc() {
        StatusRequest request = StatusRequest.newBuilder().build();
        StatusResponse response = client.getStub().status(request);
        
        System.out.println("Node ID: " + response.getNodeId());
        System.out.println("Version: " + response.getVersion());
        System.out.println("Role: " + response.getRole());
        
        if (response.hasStorage()) {
            StorageInfo storage = response.getStorage();
            System.out.println("Total vectors: " + storage.getTotalVectors());
            System.out.println("Total size: " + storage.getTotalSizeBytes() + " bytes");
        }
        
        return response;
    }
}
```

## Error Handling and Best Practices

```java
import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import java.util.concurrent.TimeUnit;

public class ProximaDBGrpcError extends RuntimeException {
    private final Status.Code statusCode;
    
    public ProximaDBGrpcError(Status.Code statusCode, String message) {
        super(statusCode.name() + ": " + message);
        this.statusCode = statusCode;
    }
    
    public Status.Code getStatusCode() {
        return statusCode;
    }
}

public class ErrorHandlingExamples {
    
    public static void handleGrpcError(StatusRuntimeException e) throws ProximaDBGrpcError {
        Status.Code code = e.getStatus().getCode();
        String details = e.getStatus().getDescription();
        
        // Map gRPC status codes to user-friendly errors
        switch (code) {
            case INVALID_ARGUMENT:
                throw new ProximaDBGrpcError(code, "Invalid request parameters: " + details);
            case NOT_FOUND:
                throw new ProximaDBGrpcError(code, "Resource not found: " + details);
            case ALREADY_EXISTS:
                throw new ProximaDBGrpcError(code, "Resource already exists: " + details);
            case UNAUTHENTICATED:
                throw new ProximaDBGrpcError(code, "Authentication failed: " + details);
            case PERMISSION_DENIED:
                throw new ProximaDBGrpcError(code, "Access denied: " + details);
            case RESOURCE_EXHAUSTED:
                throw new ProximaDBGrpcError(code, "Rate limited: " + details);
            case INTERNAL:
                throw new ProximaDBGrpcError(code, "Internal server error: " + details);
            case UNAVAILABLE:
                throw new ProximaDBGrpcError(code, "Service unavailable: " + details);
            default:
                throw new ProximaDBGrpcError(code, "Unknown error: " + details);
        }
    }
    
    // Retry with exponential backoff
    public static <T> T retryWithBackoff(RetryableOperation<T> operation, 
                                         int maxRetries, long baseDelayMs) throws Exception {
        for (int attempt = 0; attempt < maxRetries; attempt++) {
            try {
                return operation.execute();
            } catch (StatusRuntimeException e) {
                if (e.getStatus().getCode() == Status.Code.RESOURCE_EXHAUSTED 
                    && attempt < maxRetries - 1) {
                    long delay = baseDelayMs * (long) Math.pow(2, attempt);
                    System.out.println("Rate limited, retrying in " + delay + "ms...");
                    TimeUnit.MILLISECONDS.sleep(delay);
                    continue;
                }
                handleGrpcError(e);
            }
        }
        throw new Exception("Max retries exceeded");
    }
    
    @FunctionalInterface
    public interface RetryableOperation<T> {
        T execute() throws StatusRuntimeException;
    }
    
    // Usage example
    public static void main(String[] args) {
        ProximaDBGrpcClient client = new ProximaDBGrpcClient(
            "localhost", 9090, "pk_test_example123"
        );
        
        try {
            // Example with retry
            CreateCollectionResponse response = retryWithBackoff(() -> {
                CreateCollectionRequest request = CreateCollectionRequest.newBuilder()
                        .setCollectionId("test_collection")
                        .setDimension(128)
                        .build();
                return client.getStub().createCollection(request);
            }, 3, 1000);
            
            System.out.println("Collection created successfully");
            
        } catch (ProximaDBGrpcError e) {
            if (e.getStatusCode() == Status.Code.ALREADY_EXISTS) {
                System.out.println("Collection already exists");
            } else if (e.getStatusCode() == Status.Code.RESOURCE_EXHAUSTED) {
                System.out.println("Rate limited, try again later");
            } else {
                System.err.println("Error: " + e.getMessage());
            }
        } catch (Exception e) {
            System.err.println("Unexpected error: " + e.getMessage());
        } finally {
            try {
                client.shutdown();
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }
    }
}
```

## Official SDK Usage

```java
import ai.proximadb.client.ProximaDBClient;
import ai.proximadb.client.config.ClientConfig;
import ai.proximadb.client.model.*;

public class OfficialSDKExample {
    public static void main(String[] args) {
        // Using the official ProximaDB Java client
        ClientConfig config = ClientConfig.builder()
                .apiKey("pk_live_1234567890abcdef")
                .host("localhost")
                .port(5678)
                .build();
        
        try (ProximaDBClient client = new ProximaDBClient(config)) {
            // Create collection
            Collection collection = client.createCollection("sdk_vectors", 128);
            System.out.println("Collection created: " + collection.getId());
            
            // Insert vector
            VectorWithMetadata vector = VectorWithMetadata.builder()
                    .vector(Arrays.asList(0.1f, 0.2f, 0.3f, 0.4f))
                    .metadata(Map.of("document", "example.txt"))
                    .build();
            
            InsertResult result = client.insertVector("sdk_vectors", vector);
            System.out.println("Vector inserted: " + result.getVectorId());
            
            // Search vectors
            SearchParams params = SearchParams.builder()
                    .k(10)
                    .threshold(0.8f)
                    .build();
            
            SearchResponse searchResponse = client.searchVectors(
                "sdk_vectors", 
                Arrays.asList(0.1f, 0.2f, 0.3f, 0.4f), 
                params
            );
            
            System.out.println("Found " + searchResponse.getResults().size() + " results");
            
        } catch (Exception e) {
            System.err.println("Error: " + e.getMessage());
        }
    }
}
```

This comprehensive Java guide covers both REST and gRPC APIs with practical examples for building production applications with ProximaDB.