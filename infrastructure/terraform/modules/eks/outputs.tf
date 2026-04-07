# EKS Module Outputs

output "cluster_id" {
  description = "ID of the EKS cluster"
  value       = aws_eks_cluster.this.id
}

output "cluster_arn" {
  description = "ARN of the EKS cluster"
  value       = aws_eks_cluster.this.arn
}

output "cluster_endpoint" {
  description = "Endpoint of the EKS cluster"
  value       = aws_eks_cluster.this.endpoint
}

output "cluster_certificate_authority_data" {
  description = "Base64 encoded certificate data required to communicate with the cluster"
  value       = aws_eks_cluster.this.certificate_authority[0].data
  sensitive   = true
}

output "cluster_name" {
  description = "Name of the EKS cluster"
  value       = aws_eks_cluster.this.name
}

output "cluster_status" {
  description = "Status of the EKS cluster"
  value       = aws_eks_cluster.this.status
}

output "cluster_version" {
  description = "Kubernetes version of the cluster"
  value       = aws_eks_cluster.this.version
}

output "cluster_security_group_id" {
  description = "Security group ID attached to the cluster"
  value       = aws_security_group.cluster.id
}

output "cluster_iam_role_arn" {
  description = "IAM role ARN of the EKS cluster"
  value       = aws_iam_role.cluster.arn
}

output "cluster_oidc_issuer_url" {
  description = "The URL on the EKS cluster OIDC Issuer"
  value       = var.create_oidc_provider ? aws_eks_cluster.this.identity[0].oidc[0].issuer : null
}

output "cluster_oidc_provider_arn" {
  description = "ARN of the OIDC Provider for EKS"
  value       = var.create_oidc_provider ? aws_iam_openid_connect_provider.oidc[0].arn : null
}

output "worker_iam_role_arn" {
  description = "IAM role ARN of the worker nodes"
  value       = aws_iam_role.workers.arn
}

output "worker_security_group_id" {
  description = "Security group ID attached to the worker nodes"
  value       = aws_security_group.workers.id
}

output "worker_node_groups" {
  description = "Map of EKS managed node groups"
  value = {
    for name, node_group in aws_eks_node_group.main : name => {
      id        = node_group.id
      status    = node_group.status
      resources = node_group.resources
      scaling_config = node_group.scaling_config
    }
  }
}

output "worker_node_group_ids" {
  description = "List of IDs of the EKS managed node groups"
  value       = values(aws_eks_node_group.main)[*].id
}

output "cluster_encryption_config" {
  description = "Encryption configuration for the cluster"
  value = var.encryption_key_arn != null ? {
    provider    = aws_eks_cluster.this.encryption_config[0].provider[0]
    resources   = aws_eks_cluster.this.encryption_config[0].resources
  } : null
}

output "cloudwatch_log_group_name" {
  description = "Name of the CloudWatch log group for EKS control plane logs"
  value       = var.create_cloudwatch_log_group ? aws_cloudwatch_log_group.this[0].name : null
}

output "kubectl_config" {
  description = "kubectl config for the EKS cluster"
  value = <<-EOT
apiVersion: v1
clusters:
- cluster:
    certificate-authority-data: ${aws_eks_cluster.this.certificate_authority[0].data}
    server: ${aws_eks_cluster.this.endpoint}
  name: ${aws_eks_cluster.this.name}
contexts:
- context:
    cluster: ${aws_eks_cluster.this.name}
    user: ${aws_eks_cluster.this.arn}
  name: ${aws_eks_cluster.this.name}
current-context: ${aws_eks_cluster.this.name}
kind: Config
preferences: {}
users:
- name: ${aws_eks_cluster.this.arn}
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args:
      - --region
      - ${data.aws_region.current.name}
      - eks
      - get-token
      - --cluster-name
      - ${aws_eks_cluster.this.name}
EOT

  sensitive = true
}

output "region" {
  description = "AWS region"
  value       = data.aws_region.current.name
}

output "configure_kubectl" {
  description = "Command to configure kubectl locally"
  value = <<-EOT
aws eks update-kubeconfig --name ${aws_cluster eks_cluster.this.name} --region ${data.aws_region.current.name}
EOT
}
