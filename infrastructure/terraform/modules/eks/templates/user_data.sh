#!/bin/bash
# EKS Node User Data Script
# This script runs on first boot of EKS worker nodes

set -o errexit
set -o nounset
set -o pipefail

# Boto3 should be installed for bootstrap scripts
yum install -y boto3 python3-pip

# AWS IMDSv2 token
TOKEN=$(curl -X PUT "http://169.254.169.254/latest/api/token" -H "X-aws-ec2-metadata-token-ttl-seconds: 21600")

# Get instance IMDSv2 metadata
INSTANCE_ID=$(curl -H "X-aws-ec2-metadata-token: $TOKEN" http://169.254.169.254/latest/meta-data/instance-id)
INSTANCE_TYPE=$(curl -H "X-aws-ec2-metadata-token: $TOKEN" http://169.254.169.254/latest/meta-data/instance-type)
AVAILABILITY_ZONE=$(curl -H "X-aws-ec2-metadata-token: $TOKEN" http://169.254.169.254/latest/meta-data/placement/availability-zone)
LOCAL_IPV4=$(curl -H "X-aws-ec2-metadata-token: $TOKEN" http://169.254.169.254/latest/meta-data/local-ipv4)

# Log bootstrap info
echo "Bootstrapping EKS node: $INSTANCE_ID"
echo "Instance Type: $INSTANCE_TYPE"
echo "Availability Zone: $AVAILABILITY_ZONE"
echo "Local IPv4: $LOCAL_IPV4"

# Pre-bootstrap user data (if provided)
%{ if pre_bootstrap_user_data %}
${pre_bootstrap_user_data}
%{ endif }

# EKS bootstrap script
/etc/eks/bootstrap.sh \
  --b64-cluster-ca \
  --apiserver-endpoint \
  --kubelet-extra-args '%{ if kubelet_extra_args }${kubelet_extra_args}%{ endif }'

# Post-bootstrap user data (if provided)
%{ if post_bootstrap_user_data %}
${post_bootstrap_user_data}
%{ endif }

# Signal node readiness
echo "EKS node bootstrap complete"
