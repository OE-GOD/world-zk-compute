# World ZK Compute — AWS Off-Chain Deployment
#
# Deploys the complete off-chain stack:
#   - Verifier API (ECS Fargate)
#   - Proof Registry (ECS Fargate + EFS for SQLite)
#   - Proof Generator (ECS Fargate, larger instance for proving)
#   - S3 bucket with Object Lock for immutable proof storage
#   - ALB with TLS termination
#   - CloudWatch logging
#   - VPC, security groups, IAM roles
#
# Usage:
#   cd deploy/terraform
#   terraform init
#   terraform plan -var="ecr_registry=123456789012.dkr.ecr.us-east-1.amazonaws.com"
#   terraform apply

terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = var.aws_region
}

locals {
  name_prefix = "${var.project_name}-${var.environment}"
  common_tags = {
    Project     = var.project_name
    Environment = var.environment
    ManagedBy   = "terraform"
  }
}

# --------------------------------------------------------------------------
# Data Sources
# --------------------------------------------------------------------------

data "aws_availability_zones" "available" {
  state = "available"
}

data "aws_caller_identity" "current" {}

# --------------------------------------------------------------------------
# VPC
# --------------------------------------------------------------------------

resource "aws_vpc" "main" {
  cidr_block           = "10.0.0.0/16"
  enable_dns_hostnames = true
  enable_dns_support   = true

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-vpc"
  })
}

resource "aws_subnet" "public" {
  count                   = 2
  vpc_id                  = aws_vpc.main.id
  cidr_block              = "10.0.${count.index}.0/24"
  availability_zone       = data.aws_availability_zones.available.names[count.index]
  map_public_ip_on_launch = true

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-public-${count.index}"
  })
}

resource "aws_subnet" "private" {
  count             = 2
  vpc_id            = aws_vpc.main.id
  cidr_block        = "10.0.${count.index + 10}.0/24"
  availability_zone = data.aws_availability_zones.available.names[count.index]

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-private-${count.index}"
  })
}

resource "aws_internet_gateway" "main" {
  vpc_id = aws_vpc.main.id

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-igw"
  })
}

resource "aws_eip" "nat" {
  count  = var.enable_nat_gateway ? 1 : 0
  domain = "vpc"

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-nat-eip"
  })
}

resource "aws_nat_gateway" "main" {
  count         = var.enable_nat_gateway ? 1 : 0
  allocation_id = aws_eip.nat[0].id
  subnet_id     = aws_subnet.public[0].id

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-nat"
  })
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.main.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.main.id
  }

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-public-rt"
  })
}

resource "aws_route_table" "private" {
  vpc_id = aws_vpc.main.id

  dynamic "route" {
    for_each = var.enable_nat_gateway ? [1] : []
    content {
      cidr_block     = "0.0.0.0/0"
      nat_gateway_id = aws_nat_gateway.main[0].id
    }
  }

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-private-rt"
  })
}

resource "aws_route_table_association" "public" {
  count          = 2
  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

resource "aws_route_table_association" "private" {
  count          = 2
  subnet_id      = aws_subnet.private[count.index].id
  route_table_id = aws_route_table.private.id
}

# --------------------------------------------------------------------------
# Security Groups
# --------------------------------------------------------------------------

resource "aws_security_group" "alb" {
  name        = "${local.name_prefix}-alb-sg"
  description = "Security group for the Application Load Balancer"
  vpc_id      = aws_vpc.main.id

  ingress {
    description = "HTTPS from anywhere"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "HTTP from anywhere (redirect to HTTPS)"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-alb-sg"
  })
}

resource "aws_security_group" "ecs_services" {
  name        = "${local.name_prefix}-ecs-sg"
  description = "Security group for ECS Fargate services"
  vpc_id      = aws_vpc.main.id

  ingress {
    description     = "Traffic from ALB"
    from_port       = 3000
    to_port         = 3000
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }

  ingress {
    description = "Inter-service communication"
    from_port   = 3000
    to_port     = 3002
    protocol    = "tcp"
    self        = true
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-ecs-sg"
  })
}

resource "aws_security_group" "efs" {
  name        = "${local.name_prefix}-efs-sg"
  description = "Security group for EFS mount targets"
  vpc_id      = aws_vpc.main.id

  ingress {
    description     = "NFS from ECS services"
    from_port       = 2049
    to_port         = 2049
    protocol        = "tcp"
    security_groups = [aws_security_group.ecs_services.id]
  }

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-efs-sg"
  })
}

# --------------------------------------------------------------------------
# IAM Roles
# --------------------------------------------------------------------------

# ECS task execution role (used by ECS agent to pull images, write logs)
resource "aws_iam_role" "ecs_execution" {
  name = "${local.name_prefix}-ecs-execution"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Principal = {
        Service = "ecs-tasks.amazonaws.com"
      }
    }]
  })

  tags = local.common_tags
}

resource "aws_iam_role_policy_attachment" "ecs_execution" {
  role       = aws_iam_role.ecs_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

# ECS task role (used by the container application at runtime)
resource "aws_iam_role" "ecs_task" {
  name = "${local.name_prefix}-ecs-task"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Principal = {
        Service = "ecs-tasks.amazonaws.com"
      }
    }]
  })

  tags = local.common_tags
}

# Allow proof generator and registry to read/write the S3 proofs bucket
resource "aws_iam_role_policy" "s3_proofs" {
  name = "${local.name_prefix}-s3-proofs"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:ListBucket"
        ]
        Resource = [
          aws_s3_bucket.proofs.arn,
          "${aws_s3_bucket.proofs.arn}/*"
        ]
      }
    ]
  })
}

# Allow proof registry task to mount EFS
resource "aws_iam_role_policy" "efs_access" {
  name = "${local.name_prefix}-efs-access"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "elasticfilesystem:ClientMount",
          "elasticfilesystem:ClientWrite"
        ]
        Resource = aws_efs_file_system.proof_registry.arn
        Condition = {
          StringEquals = {
            "elasticfilesystem:AccessPointArn" = aws_efs_access_point.proof_registry.arn
          }
        }
      }
    ]
  })
}

# --------------------------------------------------------------------------
# CloudWatch Logs
# --------------------------------------------------------------------------

resource "aws_cloudwatch_log_group" "worldzk" {
  name              = "/ecs/${local.name_prefix}"
  retention_in_days = var.log_retention_days

  tags = local.common_tags
}

# --------------------------------------------------------------------------
# EFS (for Proof Registry SQLite persistence)
# --------------------------------------------------------------------------

resource "aws_efs_file_system" "proof_registry" {
  creation_token = "${local.name_prefix}-proof-registry"
  encrypted      = true

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-proof-registry-efs"
  })
}

resource "aws_efs_mount_target" "proof_registry" {
  count           = 2
  file_system_id  = aws_efs_file_system.proof_registry.id
  subnet_id       = aws_subnet.private[count.index].id
  security_groups = [aws_security_group.efs.id]
}

resource "aws_efs_access_point" "proof_registry" {
  file_system_id = aws_efs_file_system.proof_registry.id

  posix_user {
    uid = 1000
    gid = 1000
  }

  root_directory {
    path = "/proof-registry"
    creation_info {
      owner_uid   = 1000
      owner_gid   = 1000
      permissions = "755"
    }
  }

  tags = local.common_tags
}

# --------------------------------------------------------------------------
# S3 Bucket — Immutable Proof Storage
# --------------------------------------------------------------------------

resource "aws_s3_bucket" "proofs" {
  bucket = "${var.project_name}-proofs-${var.environment}"

  object_lock_enabled = true

  tags = merge(local.common_tags, {
    Name = "${local.name_prefix}-proofs"
  })
}

resource "aws_s3_bucket_object_lock_configuration" "proofs" {
  bucket = aws_s3_bucket.proofs.id

  rule {
    default_retention {
      mode = "GOVERNANCE"
      years = var.proof_retention_years
    }
  }
}

resource "aws_s3_bucket_versioning" "proofs" {
  bucket = aws_s3_bucket.proofs.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "proofs" {
  bucket = aws_s3_bucket.proofs.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "aws:kms"
    }
    bucket_key_enabled = true
  }
}

resource "aws_s3_bucket_public_access_block" "proofs" {
  bucket = aws_s3_bucket.proofs.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# --------------------------------------------------------------------------
# ECS Cluster
# --------------------------------------------------------------------------

resource "aws_ecs_cluster" "worldzk" {
  name = local.name_prefix

  setting {
    name  = "containerInsights"
    value = "enabled"
  }

  tags = local.common_tags
}

# --------------------------------------------------------------------------
# ALB + Listeners + Target Groups
# --------------------------------------------------------------------------

resource "aws_lb" "worldzk" {
  name               = "${var.project_name}-${var.environment}"
  internal           = false
  load_balancer_type = "application"
  subnets            = aws_subnet.public[*].id
  security_groups    = [aws_security_group.alb.id]

  tags = local.common_tags
}

# HTTPS listener (requires certificate_arn)
resource "aws_lb_listener" "https" {
  count             = var.certificate_arn != "" ? 1 : 0
  load_balancer_arn = aws_lb.worldzk.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = var.certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.verifier.arn
  }
}

# HTTP listener — redirect to HTTPS if cert is set, otherwise forward directly
resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.worldzk.arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type = var.certificate_arn != "" ? "redirect" : "forward"

    dynamic "redirect" {
      for_each = var.certificate_arn != "" ? [1] : []
      content {
        port        = "443"
        protocol    = "HTTPS"
        status_code = "HTTP_301"
      }
    }

    target_group_arn = var.certificate_arn == "" ? aws_lb_target_group.verifier.arn : null
  }
}

# Verifier target group
resource "aws_lb_target_group" "verifier" {
  name        = "${var.project_name}-verifier-${var.environment}"
  port        = 3000
  protocol    = "HTTP"
  vpc_id      = aws_vpc.main.id
  target_type = "ip"

  health_check {
    path                = "/health"
    interval            = 30
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
    matcher             = "200"
  }

  tags = local.common_tags
}

# Proof Registry target group
resource "aws_lb_target_group" "proof_registry" {
  name        = "${var.project_name}-registry-${var.environment}"
  port        = 3001
  protocol    = "HTTP"
  vpc_id      = aws_vpc.main.id
  target_type = "ip"

  health_check {
    path                = "/health"
    interval            = 30
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
    matcher             = "200"
  }

  tags = local.common_tags
}

# Path-based routing rules (on the active listener)
resource "aws_lb_listener_rule" "proof_registry" {
  listener_arn = var.certificate_arn != "" ? aws_lb_listener.https[0].arn : aws_lb_listener.http.arn
  priority     = 100

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.proof_registry.arn
  }

  condition {
    path_pattern {
      values = ["/proofs/*", "/registry/*"]
    }
  }
}

# --------------------------------------------------------------------------
# ECS Task Definitions
# --------------------------------------------------------------------------

# Verifier API
resource "aws_ecs_task_definition" "verifier" {
  family                   = "${local.name_prefix}-verifier"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.verifier_cpu
  memory                   = var.verifier_memory
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "verifier"
    image     = "${var.ecr_registry}/worldzk-verifier:${var.image_tag}"
    essential = true

    portMappings = [{
      containerPort = 3000
      protocol      = "tcp"
    }]

    environment = [
      { name = "PORT", value = "3000" },
      { name = "RUST_LOG", value = "info" },
      { name = "RATE_LIMIT_RPM", value = tostring(var.rate_limit_rpm) },
      { name = "PROOF_BUCKET", value = aws_s3_bucket.proofs.id },
      { name = "AWS_REGION", value = var.aws_region }
    ]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.worldzk.name
        "awslogs-region"        = var.aws_region
        "awslogs-stream-prefix" = "verifier"
      }
    }
  }])

  tags = local.common_tags
}

# Proof Registry — with EFS volume for SQLite
resource "aws_ecs_task_definition" "proof_registry" {
  family                   = "${local.name_prefix}-proof-registry"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.proof_registry_cpu
  memory                   = var.proof_registry_memory
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  volume {
    name = "proof-registry-data"

    efs_volume_configuration {
      file_system_id     = aws_efs_file_system.proof_registry.id
      transit_encryption = "ENABLED"
      authorization_configuration {
        access_point_id = aws_efs_access_point.proof_registry.id
        iam             = "ENABLED"
      }
    }
  }

  container_definitions = jsonencode([{
    name      = "proof-registry"
    image     = "${var.ecr_registry}/worldzk-proof-registry:${var.image_tag}"
    essential = true

    portMappings = [{
      containerPort = 3001
      protocol      = "tcp"
    }]

    mountPoints = [{
      sourceVolume  = "proof-registry-data"
      containerPath = "/data"
      readOnly      = false
    }]

    environment = [
      { name = "PORT", value = "3001" },
      { name = "RUST_LOG", value = "info" },
      { name = "DATABASE_PATH", value = "/data/registry.db" },
      { name = "PROOF_BUCKET", value = aws_s3_bucket.proofs.id },
      { name = "AWS_REGION", value = var.aws_region }
    ]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.worldzk.name
        "awslogs-region"        = var.aws_region
        "awslogs-stream-prefix" = "proof-registry"
      }
    }
  }])

  tags = local.common_tags
}

# Proof Generator — larger instance for ZK proving workloads
resource "aws_ecs_task_definition" "proof_generator" {
  family                   = "${local.name_prefix}-proof-generator"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.proof_generator_cpu
  memory                   = var.proof_generator_memory
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "proof-generator"
    image     = "${var.ecr_registry}/worldzk-proof-generator:${var.image_tag}"
    essential = true

    portMappings = [{
      containerPort = 3002
      protocol      = "tcp"
    }]

    environment = [
      { name = "PORT", value = "3002" },
      { name = "RUST_LOG", value = "info" },
      { name = "PROOF_BUCKET", value = aws_s3_bucket.proofs.id },
      { name = "AWS_REGION", value = var.aws_region },
      { name = "REGISTRY_URL", value = "http://localhost:3001" }
    ]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.worldzk.name
        "awslogs-region"        = var.aws_region
        "awslogs-stream-prefix" = "proof-generator"
      }
    }
  }])

  tags = local.common_tags
}

# --------------------------------------------------------------------------
# ECS Services
# --------------------------------------------------------------------------

resource "aws_ecs_service" "verifier" {
  name            = "${local.name_prefix}-verifier"
  cluster         = aws_ecs_cluster.worldzk.id
  task_definition = aws_ecs_task_definition.verifier.arn
  desired_count   = var.verifier_desired_count
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = aws_subnet.private[*].id
    security_groups  = [aws_security_group.ecs_services.id]
    assign_public_ip = !var.enable_nat_gateway
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.verifier.arn
    container_name   = "verifier"
    container_port   = 3000
  }

  depends_on = [aws_lb_listener.http]

  tags = local.common_tags
}

resource "aws_ecs_service" "proof_registry" {
  name            = "${local.name_prefix}-proof-registry"
  cluster         = aws_ecs_cluster.worldzk.id
  task_definition = aws_ecs_task_definition.proof_registry.arn
  desired_count   = 1 # Single instance for SQLite consistency
  launch_type     = "FARGATE"

  platform_version = "1.4.0" # Required for EFS support

  network_configuration {
    subnets          = aws_subnet.private[*].id
    security_groups  = [aws_security_group.ecs_services.id]
    assign_public_ip = !var.enable_nat_gateway
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.proof_registry.arn
    container_name   = "proof-registry"
    container_port   = 3001
  }

  depends_on = [
    aws_lb_listener.http,
    aws_efs_mount_target.proof_registry
  ]

  tags = local.common_tags
}

resource "aws_ecs_service" "proof_generator" {
  name            = "${local.name_prefix}-proof-generator"
  cluster         = aws_ecs_cluster.worldzk.id
  task_definition = aws_ecs_task_definition.proof_generator.arn
  desired_count   = var.proof_generator_desired_count
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = aws_subnet.private[*].id
    security_groups  = [aws_security_group.ecs_services.id]
    assign_public_ip = !var.enable_nat_gateway
  }

  tags = local.common_tags
}

# --------------------------------------------------------------------------
# Auto Scaling — Verifier
# --------------------------------------------------------------------------

resource "aws_appautoscaling_target" "verifier" {
  count              = var.enable_autoscaling ? 1 : 0
  max_capacity       = var.verifier_max_count
  min_capacity       = var.verifier_desired_count
  resource_id        = "service/${aws_ecs_cluster.worldzk.name}/${aws_ecs_service.verifier.name}"
  scalable_dimension = "ecs:service:DesiredCount"
  service_namespace  = "ecs"
}

resource "aws_appautoscaling_policy" "verifier_cpu" {
  count              = var.enable_autoscaling ? 1 : 0
  name               = "${local.name_prefix}-verifier-cpu-scaling"
  policy_type        = "TargetTrackingScaling"
  resource_id        = aws_appautoscaling_target.verifier[0].resource_id
  scalable_dimension = aws_appautoscaling_target.verifier[0].scalable_dimension
  service_namespace  = aws_appautoscaling_target.verifier[0].service_namespace

  target_tracking_scaling_policy_configuration {
    predefined_metric_specification {
      predefined_metric_type = "ECSServiceAverageCPUUtilization"
    }
    target_value       = 70.0
    scale_in_cooldown  = 300
    scale_out_cooldown = 60
  }
}
