# Lightweight Metering Service: Simplified Serverless Pricing Model (Phase 1)

## 1. Introduction

This document outlines a simplified approach for implementing a lightweight metering service for proximaDB, focusing on a serverless, pay-per-use model. The primary goal is to enable variable pricing based on actual consumption, allowing customers to scale proximaDB instances up and down in real-time as single-node solutions. This initial phase leverages native cloud provider services for metering and billing integration, reducing initial development complexity.

## 2. Core Principles

*   **Serverless Deployment:** proximaDB instances are deployed using serverless compute or container orchestration services (e.g., AWS Fargate/Lambda, GCP Cloud Run/Functions, Azure Container Instances/Functions).
*   **Single-Node per Customer:** Each customer is provisioned with a dedicated proximaDB instance (or a set of instances) that scales independently.
*   **Cloud-Native Metering:** Utilize the cloud provider's inherent billing mechanisms for compute resources (CPU, memory, duration) and network egress.
*   **Direct Exposure & In-Application Metering:** proximaDB instances are directly exposed. API call metering is handled by robust in-application logging within proximaDB.
*   **Focus on Execution Time & Basic API Calls:** Initial metering will concentrate on the duration proximaDB instances are active and the number of API calls made.

## 3. Architecture Overview

### 3.1. Cloud Provider Agnostic Components

*   **proximaDB Instance:** Packaged as a container image, deployable on various serverless platforms.
*   **API Client:** Interacts with the proximaDB instance directly.

### 3.2. Cloud Provider Specific Components

#### AWS

*   **Compute:** AWS Fargate (for containerized proximaDB) or AWS Lambda (if proximaDB can be adapted to a function model).
*   **Traffic Distribution:** Direct instance exposure or DNS-based routing.
*   **Logging & Monitoring:** AWS CloudWatch for collecting logs and metrics.
*   **Billing Integration:** AWS Marketplace Metering Service for reporting usage to AWS Marketplace.

#### GCP

*   **Compute:** GCP Cloud Run (for containerized proximaDB) or GCP Cloud Functions.
*   **Traffic Distribution:** Direct instance exposure or DNS-based routing.
*   **Logging & Monitoring:** GCP Cloud Logging and Cloud Monitoring.
*   **Billing Integration:** Google Cloud Billing API for reporting usage to GCP Marketplace.

#### Azure

*   **Compute:** Azure Container Instances (for containerized proximaDB) or Azure Functions.
*   **Traffic Distribution:** Direct instance exposure or DNS-based routing.
*   **Logging & Monitoring:** Azure Monitor.
*   **Billing Integration:** Azure Marketplace Metering Service for reporting usage to Azure Marketplace.

## 4. Metering Metrics (Phase 1)

### 4.1. Execution Time of Service

*   **Metric:** Duration (e.g., CPU-hours, GB-hours) that the proximaDB instance is running and serving requests.
*   **Collection:** Automatically captured by the underlying serverless compute service (Fargate, Cloud Run, Container Instances).
*   **Billing:** Directly mapped to the cloud provider's compute billing.

### 4.2. Number of Server API Calls

*   **Metric:** Total count of API requests made to the proximaDB instance.
*   **Collection:** Captured by robust in-application logging within proximaDB. These logs are then sent to cloud-native logging services (e.g., AWS CloudWatch Logs, Google Cloud Logging, Azure Monitor Logs).
*   **Roll-up:** Logs are processed (e.g., via serverless functions like AWS Lambda, GCP Cloud Functions, Azure Functions) to aggregate call counts hourly.
*   **Billing:** Reported to the respective cloud marketplace metering service.

### 4.3. Data Scanned (Simplified)

*   **Metric:** Network egress from the proximaDB instance.
*   **Collection:** Captured by the cloud provider's network billing.
*   **Billing:** Directly mapped to the cloud provider's network egress billing.
*   **Note:** This is a simplified proxy for "data scanned" in Phase 1. More granular data scanned metrics (e.g., from object storage) will be addressed in later phases.

## 5. Billing Integration

*   **Hourly Roll-up:** Usage metrics (API calls) are aggregated hourly.
*   **Cloud Marketplace APIs:**
    *   **AWS:** `MeterUsage` API call to AWS Marketplace Metering Service.
    *   **GCP:** `reportUsage` method of the `billingAccounts.skus.projects.usage` resource via Google Cloud Billing API.
    *   **Azure:** `UsageEvent` API call to Azure Marketplace Metering Service.
*   **Margin:** A 100% margin will be applied on top of the raw cloud infrastructure costs (compute, network) to cover operational overhead and profit. This will be managed within the pricing configuration of each marketplace offering.

## 6. Implementation Considerations

*   **Containerization:** Ensure proximaDB is robustly containerized for deployment on serverless platforms.
*   **Cold Starts:** Optimize proximaDB for fast startup times to minimize impact of cold starts in a serverless environment.
*   **Cost Monitoring:** Implement dashboards to monitor actual cloud costs against revenue generated.
*   **Security:** Secure proximaDB instance endpoints.

## 7. Future Phases (Briefly Mentioned)

*   **Phase 2:** Introduce more granular metering based on specific API types (graph, vector, hybrid), object store API calls, and detailed data scanned metrics.
*   **Phase 3:** Implement engine-based pricing, potentially differentiating costs based on the underlying proximaDB engine configurations or features used.

This simplified approach provides a quick path to a serverless, variable-pay model, leveraging existing cloud infrastructure and billing mechanisms. It allows for immediate market entry with a clear pricing structure, while laying the groundwork for more sophisticated metering in subsequent phases.