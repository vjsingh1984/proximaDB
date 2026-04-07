#!/bin/bash
# ProximaDB Dev Environment Deployment Script
#
# Purpose: Automated deployment of ProximaDB development environment
# Prerequisites: AWS CLI, Terraform, kubectl, Helm configured
#
# Usage: ./deploy-dev.sh [--destroy] [--skip-terraform]
#
# Options:
#   --destroy      Destroy all resources instead of creating
#   --skip-terraform Skip Terraform deployment (Helm only)
#   --help         Show this help message

set -e  # Exit on error
set -o pipefail  # Exit on pipe failure

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
ENVIRONMENT="dev"
REGION="${AWS_REGION:-us-east-1}"
TF_DIR="${PROJECT_ROOT}/infrastructure/terraform/environments/${ENVIRONMENT}"
HELM_DIR="${PROJECT_ROOT}/infrastructure/helm/proximadb"

# Functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_prerequisites() {
    log_info "Checking prerequisites..."

    # Check AWS CLI
    if ! command -v aws &> /dev/null; then
        log_error "AWS CLI not found. Please install it first."
        exit 1
    fi

    # Check Terraform
    if ! command -v terraform &> /dev/null; then
        log_error "Terraform not found. Please install it first."
        exit 1
    fi

    # Check kubectl
    if ! command -v kubectl &> /dev/null; then
        log_error "kubectl not found. Please install it first."
        exit 1
    fi

    # Check Helm
    if ! command -v helm &> /dev/null; then
        log_error "Helm not found. Please install it first."
        exit 1
    fi

    # Check AWS credentials
    if ! aws sts get-caller-identity &> /dev/null; then
        log_error "AWS credentials not configured. Run 'aws configure' first."
        exit 1
    fi

    log_success "Prerequisites check passed"
}

configure_terraform_backend() {
    log_info "Configuring Terraform backend..."

    # Check if backend file exists
    if [ -f "${TF_DIR}/backend.tf" ]; then
        log_warning "backend.tf already exists, skipping..."
        return
    fi

    # Read secrets from environment or prompt
    STATE_BUCKET="${TERRAFORM_STATE_BUCKET:-}"
    LOCK_TABLE="${TERRAFORM_LOCK_TABLE:-}"

    if [ -z "$STATE_BUCKET" ]; then
        read -p "Enter Terraform state bucket name: " STATE_BUCKET
    fi

    if [ -z "$LOCK_TABLE" ]; then
        read -p "Enter DynamoDB lock table name: " LOCK_TABLE
    fi

    # Create backend configuration
    cat > "${TF_DIR}/backend.tf" <<EOF
terraform {
  backend "s3" {
    bucket         = "${STATE_BUCKET}"
    key            = "environments/${ENVIRONMENT}/terraform.tfstate"
    region         = "${REGION}"
    encrypt        = true
    dynamodb_table = "${LOCK_TABLE}"
  }
}
EOF

    log_success "Terraform backend configured"
}

deploy_terraform() {
    log_info "Deploying Terraform infrastructure..."

    cd "${TF_DIR}"

    # Initialize Terraform
    log_info "Initializing Terraform..."
    terraform init

    # Validate configuration
    log_info "Validating Terraform configuration..."
    terraform validate

    # Plan infrastructure
    log_info "Planning infrastructure changes..."
    terraform plan -out=tfplan

    # Ask for confirmation
    log_warning "Review the plan above carefully."
    read -p "Do you want to proceed with deployment? (yes/no): " CONFIRM

    if [ "$CONFIRM" != "yes" ]; then
        log_warning "Deployment cancelled by user."
        exit 0
    fi

    # Apply infrastructure
    log_info "Applying infrastructure changes..."
    terraform apply tfplan

    log_success "Terraform infrastructure deployed successfully"
}

destroy_terraform() {
    log_info "Destroying Terraform infrastructure..."

    cd "${TF_DIR}"

    # Initialize Terraform
    terraform init

    # Ask for confirmation
    log_warning "This will destroy all infrastructure resources."
    read -p "Are you sure you want to proceed? (yes/no): " CONFIRM

    if [ "$CONFIRM" != "yes" ]; then
        log_warning "Destruction cancelled by user."
        exit 0
    fi

    # Destroy infrastructure
    terraform destroy

    log_success "Terraform infrastructure destroyed"
}

configure_kubectl() {
    log_info "Configuring kubectl for EKS cluster..."

    CLUSTER_NAME="proximadb-${ENVIRONMENT}"

    # Update kubeconfig
    aws eks update-kubeconfig \
        --name "${CLUSTER_NAME}" \
        --region "${REGION}" \
        --alias "${CLUSTER_NAME}"

    # Set current context
    kubectl config use-context "${CLUSTER_NAME}"

    # Wait for nodes to be ready
    log_info "Waiting for EKS nodes to be ready..."
    kubectl wait --for=condition=ready nodes \
        --all \
        --timeout=600s

    log_success "kubectl configured successfully"
}

deploy_helm() {
    log_info "Deploying ProximaDB via Helm..."

    # Create namespace if it doesn't exist
    kubectl create namespace proximadb --dry-run=client -o yaml | kubectl apply -f -

    # Deploy ProximaDB
    helm upgrade --install proximadb "${HELM_DIR}" \
        --namespace proximadb \
        --values "${TF_DIR}/helm-values.yaml" \
        --wait \
        --timeout 10m

    log_success "ProximaDB deployed successfully"
}

destroy_helm() {
    log_info "Uninstalling ProximaDB Helm chart..."

    helm uninstall proximadb --namespace proximadb || true

    log_success "ProximaDB uninstalled"
}

run_tests() {
    log_info "Running deployment tests..."

    # Wait for pods to be ready
    log_info "Waiting for ProximaDB pods to be ready..."
    kubectl wait --for=condition=ready pod \
        -l app=proximadb \
        -n proximadb \
        --timeout=300s

    # Get pods
    log_info "ProximaDB pods:"
    kubectl get pods -n proximadb

    # Get services
    log_info "ProximaDB services:"
    kubectl get svc -n proximadb

    # Port forward and test health endpoint
    log_info "Testing health endpoint..."
    kubectl port-forward svc/proximadb 5678:5678 -n proximadb &
    PF_PID=$!

    sleep 5

    # Test health endpoint
    if curl -f http://localhost:5678/health; then
        log_success "Health check passed"
    else
        log_error "Health check failed"
        kill $PF_PID
        exit 1
    fi

    kill $PF_PID

    # Get Load Balancer endpoint
    LB_ENDPOINT=$(kubectl get svc proximadb -n proximadb -o jsonpath='{.status.loadBalancer.ingress[0].hostname}')

    if [ -n "$LB_ENDPOINT" ]; then
        log_success "Load Balancer endpoint: http://${LB_ENDPOINT}:5678"
    fi

    log_success "All tests passed"
}

show_summary() {
    log_info "Deployment Summary"
    echo ""

    # Show cluster info
    log_info "Cluster Information:"
    kubectl cluster-info

    # Show nodes
    log_info "Cluster Nodes:"
    kubectl get nodes

    # Show ProximaDB resources
    log_info "ProximaDB Resources:"
    kubectl get all -n proximadb

    # Show HPA
    log_info "Horizontal Pod Autoscaler:"
    kubectl get hpa -n proximadb || true

    # Get Load Balancer endpoint
    LB_ENDPOINT=$(kubectl get svc proximadb -n proximadb -o jsonpath='{.status.loadBalancer.ingress[0].hostname}')

    echo ""
    log_success "Deployment completed successfully!"
    echo ""
    log_info "Next Steps:"
    echo "  1. Access ProximaDB: http://${LB_ENDPOINT}:5678"
    echo "  2. Monitor logs: kubectl logs -l app=proximadb -n proximadb --tail=100 -f"
    echo "  3. Access Grafana: kubectl port-forward svc/prometheus-grafana 3000:80 -n monitoring"
    echo "  4. Destroy environment: ./deploy-dev.sh --destroy"
}

# Parse arguments
DESTROY=false
SKIP_TERRAFORM=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --destroy)
            DESTROY=true
            shift
            ;;
        --skip-terraform)
            SKIP_TERRAFORM=true
            shift
            ;;
        --help)
            echo "Usage: $0 [--destroy] [--skip-terraform]"
            echo ""
            echo "Options:"
            echo "  --destroy      Destroy all resources instead of creating"
            echo "  --skip-terraform Skip Terraform deployment (Helm only)"
            echo "  --help         Show this help message"
            exit 0
            ;;
        *)
            log_error "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Main execution
main() {
    log_info "ProximaDB Dev Environment Deployment"
    echo ""

    check_prerequisites

    if [ "$DESTROY" = true ]; then
        destroy_helm
        destroy_terraform
        exit 0
    fi

    if [ "$SKIP_TERRAFORM" = false ]; then
        configure_terraform_backend
        deploy_terraform
    fi

    configure_kubectl
    deploy_helm
    run_tests
    show_summary
}

main
