# EKS Module for ProximaDB Cloud Service
# Creates Amazon EKS cluster with managed node groups

terraform {
  required_version = ">= 1.0"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
    kubernetes = {
      source  = "hashicorp/kubernetes"
      version = "~> 2.23"
    }
  }
}

# Get current AWS region
data "aws_region" "current" {}

# Get current AWS account ID
data "aws_caller_identity" "current" {}

# EKS Cluster
resource "aws_eks_cluster" "this" {
  name     = var.cluster_name
  role_arn = aws_iam_role.cluster.arn
  version  = var.kubernetes_version

  vpc_config {
    subnet_ids              = var.subnet_ids
    security_group_ids      = [aws_security_group.cluster.id]
    endpoint_private_access = var.endpoint_private_access
    endpoint_public_access  = var.endpoint_public_access
    public_access_cidrs     = var.public_access_cidrs
  }

  kubernetes_network_config {
    service_ipv4_cidr = var.cluster_service_ipv4_cidr
  }

  enabled_cluster_log_types = var.cluster_enabled_log_types

  encryption_config {
    provider {
      key_arn = var.encryption_key_arn
    }
    resources = var.encryption_resources
  }

  tags = merge(
    var.tags,
    {
      Name        = var.cluster_name
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  }

  depends_on = [aws_cloudwatch_log_group.this]
}

# CloudWatch Log Group for EKS Control Plane Logs
resource "aws_cloudwatch_log_group" "this" {
  count = var.create_cloudwatch_log_group ? 1 : 0

  name              = "/aws/eks/${var.cluster_name}/cluster"
  retention_in_days = var.cloudwatch_log_retention_days

  tags = merge(
    var.tags,
    {
      Name        = "${var.cluster_name}-logs"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# EKS Cluster IAM Role
resource "aws_iam_role" "cluster" {
  name = "${var.cluster_name}-cluster-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "eks.amazonaws.com"
        }
      }
    ]
  })

  tags = merge(
    var.tags,
    {
      Name        = "${var.cluster_name}-cluster-role"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# EKS Cluster IAM Policy Attachments
resource "aws_iam_role_policy_attachment" "cluster_amazon_eks_cluster_policy" {
  policy_arn = "arn:aws:iam::aws:policy/AmazonEKSClusterPolicy"
  role       = aws_iam_role.cluster.name
}

resource "aws_iam_role_policy_attachment" "cluster_amazon_eks_vpc_resource_controller" {
  policy_arn = "arn:aws:iam::aws:policy/AmazonEKSVPCResourceController"
  role       = aws_iam_role.cluster.name
}

# EKS Cluster Security Group
resource "aws_security_group" "cluster" {
  name        = "${var.cluster_name}-cluster-sg"
  description = "EKS cluster security group"
  vpc_id      = var.vpc_id

  tags = merge(
    var.tags,
    {
      Name        = "${var.cluster_name}-cluster-sg"
      ManagedBy   = "Terraform"
      Environment = var.environment
      "kubernetes.io/cluster/${var.cluster_name}" = "owned"
    }
  )
}

# EKS Cluster Security Group Rules
resource "aws_security_group_rule" "cluster_egress" {
  description       = "Allow cluster egress to anywhere"
  from_port         = 0
  to_port           = 0
  protocol          = "-1"
  cidr_blocks       = ["0.0.0.0/0"]
  security_group_id = aws_security_group.cluster.id
  type              = "egress"
}

resource "aws_security_group_rule" "cluster_https_ingress_workers" {
  description              = "Allow pods to communicate with the cluster API Server"
  from_port                = 443
  to_port                  = 443
  protocol                 = "tcp"
  source_security_group_id = aws_security_group.workers.id
  security_group_id        = aws_security_group.cluster.id
  type                     = "ingress"
}

# OIDC Provider for EKS
resource "aws_eks_cluster" "oidc" {
  depends_on = [aws_eks_cluster.this]
}

resource "aws_iam_openid_connect_provider" "oidc" {
  count = var.create_oidc_provider ? 1 : 0

  client_id_list  = ["sts.amazonaws.com"]
  thumbprint_list = var.oidc_thumbprint_list
  url             = aws_eks_cluster.this.identity[0].oidc[0].issuer
}

# EKS Node IAM Role
resource "aws_iam_role" "workers" {
  name = "${var.cluster_name}-node-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "ec2.amazonaws.com"
        }
      }
    ]
  })

  tags = merge(
    var.tags,
    {
      Name        = "${var.cluster_name}-node-role"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# EKS Node IAM Policy Attachments
resource "aws_iam_role_policy_attachment" "workers_amazon_eks_worker_node_policy" {
  policy_arn = "arn:aws:iam::aws:policy/AmazonEKSWorkerNodePolicy"
  role       = aws_iam_role.workers.name
}

resource "aws_iam_role_policy_attachment" "workers_amazon_eks_cni_policy" {
  policy_arn = "arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy"
  role       = aws_iam_role.workers.name
}

resource "aws_iam_role_policy_attachment" "workers_amazon_ec2_container_registry_read_only" {
  policy_arn = "arn:aws:iam::aws:policy/AmazonEC2ContainerRegistryReadOnly"
  role       = aws_iam_role.workers.name
}

resource "aws_iam_role_policy_attachment" "workers_additional_policies" {
  count = length(var.worker_additional_iam_policies)

  policy_arn = var.worker_additional_iam_policies[count.index]
  role       = aws_iam_role.workers.name
}

# EKS Managed Node Groups
resource "aws_eks_node_group" "main" {
  for_each = var.managed_node_groups

  cluster_name    = aws_eks_cluster.this.name
  node_group_name = each.key
  node_role_arn   = aws_iam_role.workers.arn
  subnet_ids      = var.subnet_ids

  scaling_config {
    desired_size = each.value.desired_size
    max_size     = each.value.max_size
    min_size     = each.value.min_size
  }

  instance_types = each.value.instance_types

  # Capacity type can be SPOT or ON_DEMAND
  capacity_type  = each.value.capacity_type

  # Disk size for node instances
  disk_size = each.value.disk_size
  disk_type = each.value.disk_type

  # Enable IMDSv2 for improved security
  instance_types = each.value.instance_types

  # Labels for node group
  labels = merge(
    each.value.labels,
    {
      "NodeGroup" = each.key
    }
  )

  # Taints for node group
  dynamic "taint" {
    for_each = each.value.taints
    content {
      key    = taint.value.key
      value  = taint.value.value
      effect = taint.value.effect
    }
  }

  # Launch template configuration
  launch_template {
    name    = "${var.cluster_name}-${each.key}-lt"
    version = aws_launch_template.workers[each.key].latest_version

    # Enable IMDSv2
    metadata_options {
      http_endpoint               = "enabled"
      http_tokens                 = "required"
      http_put_response_hop_limit = 2
    }
  }

  # Update configuration
  update_config {
    max_unavailable_percentage = each.value.max_unavailable_percentage
    max_unavailable_count      = each.value.max_unavailable_count
  }

  # Ensure nodes are created before cluster is considered ready
  depends_on = [
    aws_iam_role_policy_attachment.workers_amazon_eks_worker_node_policy,
    aws_iam_role_policy_attachment.workers_amazon_eks_cni_policy,
    aws_iam_role_policy_attachment.workers_amazon_ec2_container_registry_read_only,
  ]

  tags = merge(
    var.tags,
    each.value.tags,
    {
      Name        = "${var.cluster_name}-${each.key}"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )

  # Ensure node group is created before cluster is used
  lifecycle {
    ignore_changes = [scaling_config[0].desired_size]
  }
}

# Launch Template for Node Groups
resource "aws_launch_template" "workers" {
  for_each = var.managed_node_groups

  name_prefix = "${var.cluster_name}-${each.key}-"
  description = "Launch template for ${each.key} node group"

  image_id    = each.value.custom_ami_id != null ? each.value.custom_ami_id : data.aws_eks_ssm_image.workers[each.key].id
  instance_type = each.value.instance_types[0]
  key_name     = each.value.ssh_key_name

  monitoring {
    enabled = true
  }

  network_interfaces {
    associate_public_ip_address = false
    security_groups             = concat([aws_security_group.workers.id], var.worker_security_group_ids)
    delete_on_termination       = false
  }

  block_device_mappings {
    device_name = "/dev/xvda"

    ebs {
      volume_type           = each.value.disk_type
      volume_size           = each.value.disk_size
      encrypted             = true
      delete_on_termination = true
    }
  }

  tag_specifications {
    resource_type = "instance"

    tags = merge(
      var.tags,
      each.value.tags,
      {
        Name                               = "${var.cluster_name}-${each.key}"
        "kubernetes.io/cluster/${var.cluster_name}" = "owned"
        ManagedBy                           = "Terraform"
        Environment                         = var.environment
      }
    )
  }

  user_data = base64encode(templatefile("${path.module}/templates/user_data.sh", {
    cluster_name        = aws_eks_cluster.this.name
    cluster_endpoint    = aws_eks_cluster.this.endpoint
    cluster_auth_base64 = aws_eks_cluster.this.certificate_authority[0].data
    pre_bootstrap_user_data = each.value.pre_bootstrap_user_data
    post_bootstrap_user_data = each.value.post_bootstrap_user_data
    kubelet_extra_args = each.value.kubelet_extra_args
  }))

  update_default_version = true

  lifecycle {
    create_before_destroy = true
  }
}

# Get latest EKS optimized AMI
data "aws_eks_ssm_image" "workers" {
  for_each = var.managed_node_groups

  kubernetes_version = var.kubernetes_version
  platform           = each.value.platform
}

# Workers Security Group
resource "aws_security_group" "workers" {
  name        = "${var.cluster_name}-worker-sg"
  description = "Security group for all worker nodes"
  vpc_id      = var.vpc_id

  tags = merge(
    var.tags,
    {
      Name        = "${var.cluster_name}-worker-sg"
      ManagedBy   = "Terraform"
      Environment = var.environment
      "kubernetes.io/cluster/${var.cluster_name}" = "owned"
    }
  )
}

# Workers Security Group Rules
resource "aws_security_group_rule" "workers_egress" {
  description       = "Allow nodes to communicate with any destination"
  from_port         = 0
  to_port           = 0
  protocol          = "-1"
  cidr_blocks       = ["0.0.0.0/0"]
  security_group_id = aws_security_group.workers.id
  type              = "egress"
}

resource "aws_security_group_rule" "workers_ingress_self" {
  description                = "Allow nodes to communicate with each other"
  from_port                  = 0
  to_port                    = 65535
  protocol                   = "-1"
  source_security_group_id   = aws_security_group.workers.id
  security_group_id          = aws_security_group.workers.id
  type                       = "ingress"
}

resource "aws_security_group_rule" "workers_ingress_cluster" {
  description              = "Allow worker Kubelets and pods to receive communication from the cluster control plane"
  from_port                = 1025
  to_port                  = 65535
  protocol                 = "tcp"
  source_security_group_id = aws_security_group.cluster.id
  security_group_id        = aws_security_group.workers.id
  type                     = "ingress"
}

# Kubernetes provider configuration
data "aws_eks_cluster_auth" "this" {
  name = aws_eks_cluster.this.name
}

provider "kubernetes" {
  host                   = aws_eks_cluster.this.endpoint
  cluster_ca_certificate = base64decode(aws_eks_cluster.this.certificate_authority[0].data)
  token                  = data.aws_eks_cluster_auth.this.token
}

# Wait for cluster to be ready
resource "time_sleep" "this" {
  create_duration = var.cluster_create_wait_duration

  depends_on = [aws_eks_cluster.this]
}

# Kubernetes ConfigMap for CoreDNS
resource "kubernetes_config_map" "coredns" {
  count = var.create_coredns_configmap ? 1 : 0

  metadata {
    name      = "coredns"
    namespace = "kube-system"
  }

  data = {
    render = "coredns"
  }

  depends_on = [time_sleep.this]
}

# Kubernetes ConfigMap for kube-proxy
resource "kubernetes_config_map" "kube_proxy" {
  count = var.create_kube_proxy_configmap ? 1 : 0

  metadata {
    name      = "kube-proxy"
    namespace = "kube-system"
  }

  data = {
    render = "kube-proxy"
  }

  depends_on = [time_sleep.this]
}
