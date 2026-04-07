# VPC Module for ProximaDB Cloud Service
# Creates a complete network infrastructure with public and private subnets

terraform {
  required_version = ">= 1.0"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

# Get available AWS availability zones
data "aws_availability_zones" "available" {
  state = "available"
}

# VPC
resource "aws_vpc" "this" {
  cidr_block           = var.vpc_cidr
  enable_dns_hostnames = true
  enable_dns_support   = true

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-vpc"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# Public subnets
resource "aws_subnet" "public" {
  count = var.availability_zone_count

  vpc_id                  = aws_vpc.this.id
  cidr_block              = cidrsubnet(var.vpc_cidr, 8, count.index)
  availability_zone       = data.aws_availability_zones.available.names[count.index]
  map_public_ip_on_launch = true

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-public-${data.aws_availability_zones.available.names[count.index]}"
      Type        = "public"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# Private subnets (for EKS nodes)
resource "aws_subnet" "private" {
  count = var.availability_zone_count

  vpc_id            = aws_vpc.this.id
  cidr_block         = cidrsubnet(var.vpc_cidr, 8, count.index + var.availability_zone_count)
  availability_zone  = data.aws_availability_zones.available.names[count.index]

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-private-${data.aws_availability_zones.available.names[count.index]}"
      Type        = "private"
      ManagedBy   = "Terraform"
      Environment = var.environment
      "kubernetes.io/role/elb" = "1"
    }
  )
}

# Database subnets (isolated for RDS)
resource "aws_subnet" "database" {
  count = var.availability_zone_count

  vpc_id           = aws_vpc.this.id
  cidr_block        = cidrsubnet(var.vpc_cidr, 8, count.index + var.availability_zone_count * 2)
  availability_zone = data.aws_availability_zones.available.names[count.index]

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-database-${data.aws_availability_zones.available.names[count.index]}"
      Type        = "database"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# Elastic IP for NAT Gateway
resource "aws_eip" "nat" {
  count = var.single_nat_gateway ? 1 : var.availability_zone_count

  vpc = true

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-nat-eip-${count.index}"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )

  depends_on = [aws_internet_gateway.this]
}

# NAT Gateway
resource "aws_nat_gateway" "this" {
  count = var.single_nat_gateway ? 1 : var.availability_zone_count

  allocation_id = aws_eip.nat[count.index].id
  subnet_id     = aws_subnet.public[count.index].id

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-nat-${count.index}"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )

  depends_on = [aws_internet_gateway.this]
}

# Internet Gateway
resource "aws_internet_gateway" "this" {
  vpc_id = aws_vpc.this.id

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-igw"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# Public route table
resource "aws_route_table" "public" {
  vpc_id = aws_vpc.this.id

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-public-rt"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# Public route to internet gateway
resource "aws_route" "public_internet_gateway" {
  route_table_id         = aws_route_table.public.id
  destination_cidr_block = "0.0.0.0/0"
  gateway_id             = aws_internet_gateway.this.id

  timeouts {
    create = "5m"
  }
}

# Public route table associations
resource "aws_route_table_association" "public" {
  count = var.availability_zone_count

  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

# Private route tables
resource "aws_route_table" "private" {
  count = var.single_nat_gateway ? 1 : var.availability_zone_count

  vpc_id = aws_vpc.this.id

  tags = merge(
    var.tags,
    {
      Name        = var.single_nat_gateway ? "${var.name}-private-rt" : "${var.name}-private-rt-${count.index}"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# Private routes to NAT Gateway
resource "aws_route" "private_nat_gateway" {
  count = var.single_nat_gateway ? 1 : var.availability_zone_count

  route_table_id         = aws_route_table.private[count.index].id
  destination_cidr_block = "0.0.0.0/0"
  nat_gateway_id         = aws_nat_gateway.this[count.index].id

  timeouts {
    create = "5m"
  }
}

# Private route table associations
resource "aws_route_table_association" "private" {
  count = var.availability_zone_count

  subnet_id      = aws_subnet.private[count.index].id
  route_table_id = aws_route_table.private[var.single_nat_gateway ? 0 : count.index].id
}

# Database route table (no internet access)
resource "aws_route_table" "database" {
  vpc_id = aws_vpc.this.id

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-database-rt"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# Database route table associations
resource "aws_route_table_association" "database" {
  count = var.availability_zone_count

  subnet_id      = aws_subnet.database[count.index].id
  route_table_id = aws_route_table.database.id
}

# VPC Flow Logs (for security monitoring)
resource "aws_flow_log" "this" {
  count = var.enable_flow_logs ? 1 : 0

  iam_role_arn    = aws_iam_role.vpc_flow_log[0].arn
  log_destination = aws_cloudwatch_log_group.vpc_flow_log[0].arn
  traffic_type    = "ALL"
  vpc_id          = aws_vpc.this.id

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-flow-logs"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# IAM role for VPC Flow Logs
resource "aws_iam_role" "vpc_flow_log" {
  count = var.enable_flow_logs ? 1 : 0

  name = "${var.name}-vpc-flow-log-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "vpc-flow-logs.amazonaws.com"
        }
      }
    ]
  })
}

# IAM policy for VPC Flow Logs
resource "aws_iam_role_policy" "vpc_flow_log" {
  count = var.enable_flow_logs ? 1 : 0

  name = "${var.name}-vpc-flow-log-policy"
  role = aws_iam_role.vpc_flow_log[0].id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = [
          "logs:CreateLogGroup",
          "logs:CreateLogStream",
          "logs:PutLogEvents",
          "logs:DescribeLogGroups",
          "logs:DescribeLogStreams"
        ]
        Effect   = "Allow"
        Resource = "*"
      }
    ]
  })
}

# CloudWatch Log Group for VPC Flow Logs
resource "aws_cloudwatch_log_group" "vpc_flow_log" {
  count = var.enable_flow_logs ? 1 : 0

  name              = "/aws/vpc/flow-logs/${var.name}"
  retention_in_days = var.flow_log_retention_days

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-vpc-flow-logs"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# DHCP Options (optional DNS configuration)
resource "aws_vpc_dhcp_options" "this" {
  count = var.enable_dhcp_options ? 1 : 0

  domain_name_servers = var.dhcp_domain_name_servers
  domain_name         = var.dhcp_domain_name
  ntp_servers         = var.dhcp_ntp_servers
  netbios_name_servers = var.dhcp_netbios_name_servers
  netbios_node_type   = var.dhcp_netbios_node_type

  tags = merge(
    var.tags,
    {
      Name        = "${var.name}-dhcp-options"
      ManagedBy   = "Terraform"
      Environment = var.environment
    }
  )
}

# DHCP Options Association
resource "aws_vpc_dhcp_options_association" "this" {
  count = var.enable_dhcp_options ? 1 : 0

  vpc_id          = aws_vpc.this.id
  dhcp_options_id = aws_vpc_dhcp_options.this[0].id
}
