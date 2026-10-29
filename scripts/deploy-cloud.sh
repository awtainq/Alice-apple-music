#!/usr/bin/env bash
set -euo pipefail
: "${PROJECT_ID:?Set PROJECT_ID}"
: "${SKILL_ID:?Set SKILL_ID}"
REGION="${REGION:-europe-west1}"
DATABASE="${DATABASE:-alice-music}"
PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
gcloud run deploy alice-music --source "$PROJECT_DIR/cloud" \
  --project "$PROJECT_ID" --region "$REGION" \
  --service-account "alice-music-webhook@$PROJECT_ID.iam.gserviceaccount.com" \
  --allow-unauthenticated --min-instances 0 --max-instances 2 --concurrency 8 --timeout 5 \
  --set-env-vars "GOOGLE_CLOUD_PROJECT=$PROJECT_ID,FIRESTORE_DATABASE=$DATABASE,ALICE_SKILL_ID=$SKILL_ID" \
  --set-secrets 'WEBHOOK_TOKEN=alice-music-hook:latest,AGENT_TOKEN=alice-music-agent:latest,BIND_CODE=alice-music-bind:latest'
