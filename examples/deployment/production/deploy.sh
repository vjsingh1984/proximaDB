#!/bin/bash

# Copyright 2025 ProximaDB
# Production deployment script for ProximaDB

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
DEPLOYMENT_ENV="${DEPLOYMENT_ENV:-production}"
PROXIMADB_VERSION="${PROXIMADB_VERSION:-latest}"
NAMESPACE="${NAMESPACE:-proximadb}"
RELEASE_NAME="${RELEASE_NAME:-proximadb}"

# Print colored output
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check prerequisites
check_prerequisites() {
    print_status "Checking prerequisites..."
    
    # Check if kubectl is installed
    if ! command -v kubectl &> /dev/null; then
        print_error "kubectl is not installed or not in PATH"
        exit 1
    fi
    
    # Check if helm is installed
    if ! command -v helm &> /dev/null; then
        print_error "Helm is not installed or not in PATH"
        exit 1
    fi
    
    # Check if docker is installed (for building images)
    if ! command -v docker &> /dev/null; then
        print_error "Docker is not installed or not in PATH"
        exit 1
    fi
    
    # Check kubectl connection
    if ! kubectl cluster-info &> /dev/null; then
        print_error "Cannot connect to Kubernetes cluster"
        exit 1
    fi
    
    print_success "Prerequisites check passed"
}

# Build Docker images
build_images() {
    print_status "Building ProximaDB Docker images..."
    
    cd "$PROJECT_ROOT"
    
    # Build main server image
    docker build -t "proximadb/server:${PROXIMADB_VERSION}" \
        -f docker/Dockerfile.server .
    
    # Build CLI image
    docker build -t "proximadb/cli:${PROXIMADB_VERSION}" \
        -f docker/Dockerfile.cli .
    
    # Build monitoring image
    docker build -t "proximadb/monitoring:${PROXIMADB_VERSION}" \
        -f docker/Dockerfile.monitoring .
    
    print_success "Docker images built successfully"
}

# Push images to registry
push_images() {
    local registry="${DOCKER_REGISTRY:-}"
    
    if [[ -z "$registry" ]]; then
        print_warning "DOCKER_REGISTRY not set, skipping image push"
        return 0
    fi
    
    print_status "Pushing images to registry: $registry"
    
    # Tag and push images
    for image in server cli monitoring; do
        local local_tag="proximadb/${image}:${PROXIMADB_VERSION}"
        local remote_tag="${registry}/proximadb/${image}:${PROXIMADB_VERSION}"
        
        docker tag "$local_tag" "$remote_tag"
        docker push "$remote_tag"
    done
    
    print_success "Images pushed to registry"
}

# Create namespace
create_namespace() {
    print_status "Creating namespace: $NAMESPACE"
    
    if kubectl get namespace "$NAMESPACE" &> /dev/null; then
        print_warning "Namespace $NAMESPACE already exists"
    else
        kubectl create namespace "$NAMESPACE"
        print_success "Namespace $NAMESPACE created"
    fi
}

# Deploy using Helm
deploy_with_helm() {
    print_status "Deploying ProximaDB using Helm..."
    
    cd "$PROJECT_ROOT"
    
    # Check if Helm chart exists
    if [[ ! -d "helm/proximadb" ]]; then
        print_error "Helm chart not found at helm/proximadb"
        exit 1
    fi
    
    # Install or upgrade the release
    helm upgrade --install "$RELEASE_NAME" helm/proximadb \
        --namespace "$NAMESPACE" \
        --create-namespace \
        --set image.tag="$PROXIMADB_VERSION" \
        --set environment="$DEPLOYMENT_ENV" \
        --values "helm/proximadb/values-${DEPLOYMENT_ENV}.yaml" \
        --wait \
        --timeout=10m
    
    print_success "ProximaDB deployed successfully"
}

# Deploy monitoring stack
deploy_monitoring() {
    print_status "Deploying monitoring stack..."
    
    # Deploy Prometheus
    helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
    helm repo update
    
    helm upgrade --install prometheus prometheus-community/kube-prometheus-stack \
        --namespace monitoring \
        --create-namespace \
        --set prometheus.prometheusSpec.serviceMonitorSelectorNilUsesHelmValues=false \
        --set prometheus.prometheusSpec.podMonitorSelectorNilUsesHelmValues=false \
        --wait
    
    # Deploy Grafana dashboards
    kubectl apply -f "$PROJECT_ROOT/k8s/monitoring/" -n monitoring
    
    print_success "Monitoring stack deployed"
}

# Setup storage
setup_storage() {
    print_status "Setting up storage..."
    
    # Create storage classes if they don't exist
    kubectl apply -f "$PROJECT_ROOT/k8s/storage/"
    
    # Create persistent volumes
    if [[ "$DEPLOYMENT_ENV" == "production" ]]; then
        kubectl apply -f "$PROJECT_ROOT/k8s/storage/production-pv.yaml"
    fi
    
    print_success "Storage setup completed"
}

# Configure networking
setup_networking() {
    print_status "Setting up networking..."
    
    # Apply network policies
    kubectl apply -f "$PROJECT_ROOT/k8s/networking/" -n "$NAMESPACE"
    
    # Setup ingress
    if [[ "$DEPLOYMENT_ENV" == "production" ]]; then
        kubectl apply -f "$PROJECT_ROOT/k8s/ingress/production-ingress.yaml" -n "$NAMESPACE"
    else
        kubectl apply -f "$PROJECT_ROOT/k8s/ingress/staging-ingress.yaml" -n "$NAMESPACE"
    fi
    
    print_success "Networking setup completed"
}

# Run health checks
health_check() {
    print_status "Running health checks..."
    
    # Wait for pods to be ready
    kubectl wait --for=condition=ready pod -l app=proximadb --timeout=300s -n "$NAMESPACE"
    
    # Check service endpoints
    local service_ip
    service_ip=$(kubectl get svc proximadb -n "$NAMESPACE" -o jsonpath='{.status.loadBalancer.ingress[0].ip}')
    
    if [[ -z "$service_ip" ]]; then
        service_ip=$(kubectl get svc proximadb -n "$NAMESPACE" -o jsonpath='{.spec.clusterIP}')
    fi
    
    print_status "Testing connectivity to $service_ip"
    
    # Test health endpoint
    if kubectl run test-connectivity --rm -i --restart=Never --image=curlimages/curl -- \
        curl -f "http://$service_ip:5678/health" &> /dev/null; then
        print_success "Health check passed"
    else
        print_error "Health check failed"
        return 1
    fi
}

# Setup SSL certificates
setup_ssl() {
    if [[ "$DEPLOYMENT_ENV" != "production" ]]; then
        print_warning "Skipping SSL setup for non-production environment"
        return 0
    fi
    
    print_status "Setting up SSL certificates..."
    
    # Install cert-manager if not already installed
    if ! kubectl get crd certificates.cert-manager.io &> /dev/null; then
        helm repo add jetstack https://charts.jetstack.io
        helm repo update
        
        helm install cert-manager jetstack/cert-manager \
            --namespace cert-manager \
            --create-namespace \
            --version v1.13.0 \
            --set installCRDs=true
    fi
    
    # Apply certificate resources
    kubectl apply -f "$PROJECT_ROOT/k8s/certificates/" -n "$NAMESPACE"
    
    print_success "SSL certificates configured"
}

# Setup secrets
setup_secrets() {
    print_status "Setting up secrets..."
    
    # Create secrets from environment variables or files
    if [[ -f "$PROJECT_ROOT/secrets/${DEPLOYMENT_ENV}-secrets.yaml" ]]; then
        kubectl apply -f "$PROJECT_ROOT/secrets/${DEPLOYMENT_ENV}-secrets.yaml" -n "$NAMESPACE"
    else
        print_warning "No secrets file found for $DEPLOYMENT_ENV environment"
    fi
    
    print_success "Secrets configured"
}

# Backup existing deployment
backup_deployment() {
    if [[ "$DEPLOYMENT_ENV" != "production" ]]; then
        print_warning "Skipping backup for non-production environment"
        return 0
    fi
    
    print_status "Creating backup of existing deployment..."
    
    local backup_dir="$PROJECT_ROOT/backups/$(date +%Y%m%d_%H%M%S)"
    mkdir -p "$backup_dir"
    
    # Backup Helm release
    helm get values "$RELEASE_NAME" -n "$NAMESPACE" > "$backup_dir/helm-values.yaml"
    
    # Backup persistent volumes
    kubectl get pv -o yaml > "$backup_dir/persistent-volumes.yaml"
    
    print_success "Backup created at $backup_dir"
}

# Rollback deployment
rollback_deployment() {
    print_status "Rolling back deployment..."
    
    helm rollback "$RELEASE_NAME" -n "$NAMESPACE"
    
    print_success "Deployment rolled back"
}

# Clean up resources
cleanup() {
    print_status "Cleaning up resources..."
    
    # Delete the Helm release
    helm uninstall "$RELEASE_NAME" -n "$NAMESPACE" || true
    
    # Delete namespace
    kubectl delete namespace "$NAMESPACE" || true
    
    print_success "Cleanup completed"
}

# Display deployment info
show_deployment_info() {
    print_success "ProximaDB deployment completed!"
    echo
    echo "Deployment Information:"
    echo "======================"
    echo "Environment: $DEPLOYMENT_ENV"
    echo "Namespace: $NAMESPACE"
    echo "Release: $RELEASE_NAME"
    echo "Version: $PROXIMADB_VERSION"
    echo
    
    # Get service information
    echo "Services:"
    kubectl get svc -n "$NAMESPACE"
    echo
    
    # Get pod information
    echo "Pods:"
    kubectl get pods -n "$NAMESPACE"
    echo
    
    # Get ingress information
    echo "Ingress:"
    kubectl get ingress -n "$NAMESPACE" || echo "No ingress configured"
    echo
    
    print_status "To access ProximaDB:"
    local service_ip
    service_ip=$(kubectl get svc proximadb -n "$NAMESPACE" -o jsonpath='{.status.loadBalancer.ingress[0].ip}' 2>/dev/null || echo "")
    
    if [[ -n "$service_ip" ]]; then
        echo "External IP: http://$service_ip:5678"
    else
        echo "Port forward: kubectl port-forward svc/proximadb 5678:5678 -n $NAMESPACE"
    fi
    
    print_status "To view logs:"
    echo "kubectl logs -f deployment/proximadb -n $NAMESPACE"
    
    print_status "To scale deployment:"
    echo "kubectl scale deployment proximadb --replicas=3 -n $NAMESPACE"
}

# Main deployment function
main() {
    local action="${1:-deploy}"
    
    case "$action" in
        "deploy")
            print_status "Starting ProximaDB deployment..."
            check_prerequisites
            backup_deployment
            create_namespace
            setup_secrets
            setup_storage
            build_images
            push_images
            deploy_with_helm
            setup_networking
            setup_ssl
            deploy_monitoring
            health_check
            show_deployment_info
            ;;
        "rollback")
            rollback_deployment
            ;;
        "cleanup")
            cleanup
            ;;
        "health")
            health_check
            ;;
        *)
            echo "Usage: $0 {deploy|rollback|cleanup|health}"
            echo
            echo "Commands:"
            echo "  deploy   - Deploy ProximaDB to Kubernetes"
            echo "  rollback - Rollback to previous deployment"
            echo "  cleanup  - Remove ProximaDB deployment"
            echo "  health   - Run health checks"
            echo
            echo "Environment Variables:"
            echo "  DEPLOYMENT_ENV    - Deployment environment (production, staging, development)"
            echo "  PROXIMADB_VERSION - ProximaDB version to deploy"
            echo "  NAMESPACE         - Kubernetes namespace"
            echo "  RELEASE_NAME      - Helm release name"
            echo "  DOCKER_REGISTRY   - Docker registry for images"
            exit 1
            ;;
    esac
}

# Run main function with all arguments
main "$@"