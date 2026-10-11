import { z } from "zod";

//#region src/router.d.ts
//#region src/router.d.ts
type Context = {
  /** Raw Authorization header value (e.g. "Bearer <token>") */authorization: string | null;
};
declare const appRouter: import("@trpc/server").TRPCBuiltRouter<{
  ctx: Context;
  meta: object;
  errorShape: import("@trpc/server").TRPCDefaultErrorShape;
  transformer: false;
}, import("@trpc/server").TRPCDecorateCreateRouterOptions<{
  getUploadUrl: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      id: string;
      contentType?: string | undefined;
      action?: "write" | "resumable" | undefined;
    };
    output: {
      uploadUrl: string;
      fileRef: {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId: string;
      };
    } | {
      uploadUrl: null;
      fileRef: {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId: string;
      };
    };
    meta: object;
  }>;
  analyze: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      media: {
        kind: "url";
        url: string;
        assetId?: string | undefined;
      } | {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId?: string | undefined;
      };
      prompt?: string | undefined;
    };
    output: {
      analysis: string;
      generationId: string | null;
    };
    meta: object;
  }>;
  generateImage: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      model: string;
      prompt: string;
      negativePrompt?: string | undefined;
      seed?: number | undefined;
      images?: ({
        kind: "url";
        url: string;
        assetId?: string | undefined;
      } | {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId?: string | undefined;
      })[] | undefined;
      aspectRatio?: "1:1" | "4:3" | "3:4" | "16:9" | "9:16" | "21:9" | undefined;
      count?: number | undefined;
    };
    output: {
      name: string;
      results: GenerationResult[];
      generationId: string | null;
    };
    meta: object;
  }>;
  generateVideo: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      model: string;
      prompt: string;
      negativePrompt?: string | undefined;
      seed?: number | undefined;
      startFrame?: {
        kind: "url";
        url: string;
        assetId?: string | undefined;
      } | {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId?: string | undefined;
      } | undefined;
      endFrame?: {
        kind: "url";
        url: string;
        assetId?: string | undefined;
      } | {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId?: string | undefined;
      } | undefined;
      aspectRatio?: "1:1" | "4:3" | "3:4" | "16:9" | "9:16" | undefined;
      duration?: number | undefined;
      generateAudio?: boolean | undefined;
    };
    output: {
      name: string;
      results: GenerationResult[];
      generationId: string | null;
    };
    meta: object;
  }>;
  generateSound: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      model: string;
      prompt: string;
      negativePrompt?: string | undefined;
      seed?: number | undefined;
      duration?: number | undefined;
    };
    output: {
      name: string;
      results: GenerationResult[];
      generationId: string | null;
    };
    meta: object;
  }>;
  textToSpeech: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      model: string;
      prompt: string;
      negativePrompt?: string | undefined;
      seed?: number | undefined;
      voice?: string | undefined;
      outputFormat?: "mp3" | "wav" | "ogg" | "flac" | undefined;
    };
    output: {
      name: string;
      results: GenerationResult[];
      generationId: string | null;
    };
    meta: object;
  }>;
  removeBackground: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      image: {
        kind: "url";
        url: string;
        assetId?: string | undefined;
      } | {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId?: string | undefined;
      };
    };
    output: {
      url: string;
      generationId: string | null;
    };
    meta: object;
  }>;
  upscaleImage: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      image: {
        kind: "url";
        url: string;
        assetId?: string | undefined;
      } | {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId?: string | undefined;
      };
    };
    output: {
      url: string;
      generationId: string | null;
    };
    meta: object;
  }>;
  upscaleVideo: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      video: {
        kind: "url";
        url: string;
        assetId?: string | undefined;
      } | {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId?: string | undefined;
      };
    };
    output: {
      url: string;
      generationId: string | null;
    };
    meta: object;
  }>;
  addAudioToVideo: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      video: {
        kind: "url";
        url: string;
        assetId?: string | undefined;
      } | {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId?: string | undefined;
      };
    };
    output: {
      url: string;
      generationId: string | null;
    };
    meta: object;
  }>;
  transcribe: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      audio: {
        kind: "url";
        url: string;
        assetId?: string | undefined;
      } | {
        kind: "gcs";
        bucket: string;
        object: string;
        assetId?: string | undefined;
      };
      languageCode?: string | undefined;
    };
    output: {
      results: {
        text: string;
        words: {
          text: string;
          start: number;
          end: number;
        }[];
      }[];
      generationId: string | null;
    };
    meta: object;
  }>;
  getBillingInfo: import("@trpc/server").TRPCQueryProcedure<{
    input: void;
    output: {
      email: string;
      address: {
        legalName: string;
        line1: string;
        line2: string;
        city: string;
        state: string;
        postalCode: string;
        country: string;
      } | null;
      taxId: string | null;
      paymentMethod: {
        brand: string;
        last4: string;
        expMonth: number;
        expYear: number;
      } | null;
      billingPeriod: "month" | "year" | null;
    };
    meta: object;
  }>;
  listInvoices: import("@trpc/server").TRPCQueryProcedure<{
    input: void;
    output: {
      id: string;
      status: InvoiceStatus | null;
      total: number;
      currency: string;
      dueDate: number;
      hostedUrl: string | null;
    }[];
    meta: object;
  }>;
  createBillingPortal: import("@trpc/server").TRPCMutationProcedure<{
    input: void;
    output: {
      url: string;
    };
    meta: object;
  }>;
  createSubscription: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      creditQuantity: "2_500" | "5_000" | "15_000" | "30_000" | "45_000";
      billingPeriod: "month" | "year";
      successUrl: string;
      cancelUrl: string;
    };
    output: {
      url: string;
    };
    meta: object;
  }>;
  createTopup: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      creditQuantity: "5_000" | "1_000" | "2_000" | "10_000" | "25_000";
      successUrl: string;
      cancelUrl: string;
    };
    output: {
      url: string;
    };
    meta: object;
  }>;
  getSubscriptionSummary: import("@trpc/server").TRPCQueryProcedure<{
    input: void;
    output: {
      amount: number;
      currency: string;
      billingPeriod: "month" | "year";
      currentPeriodEnd: number;
    } | null;
    meta: object;
  }>;
  updateEmailPreference: import("@trpc/server").TRPCMutationProcedure<{
    input: {
      column: "product_updates_enabled" | "marketing_announcements_enabled";
      value: boolean;
    };
    output: {
      success: boolean;
    };
    meta: object;
  }>;
  deleteAccount: import("@trpc/server").TRPCMutationProcedure<{
    input: void;
    output: {
      success: boolean;
    };
    meta: object;
  }>;
}>>;
type AppRouter = typeof appRouter; //#endregion
//#endregion
//#region src/schemas.d.ts
declare const fileRef: z.ZodDiscriminatedUnion<[z.ZodObject<{
  kind: z.ZodLiteral<"url">;
  url: z.ZodString;
  assetId: z.ZodOptional<z.ZodString>;
}, z.core.$strip>, z.ZodObject<{
  kind: z.ZodLiteral<"gcs">;
  bucket: z.ZodString;
  object: z.ZodString;
  assetId: z.ZodOptional<z.ZodString>;
}, z.core.$strip>], "kind">;
declare const baseGenerationInput: z.ZodObject<{
  model: z.ZodString;
  prompt: z.ZodString;
  negativePrompt: z.ZodOptional<z.ZodString>;
  seed: z.ZodOptional<z.ZodNumber>;
}, z.core.$strip>;
declare const imageGenerationInput: z.ZodObject<{
  model: z.ZodString;
  prompt: z.ZodString;
  negativePrompt: z.ZodOptional<z.ZodString>;
  seed: z.ZodOptional<z.ZodNumber>;
  images: z.ZodOptional<z.ZodArray<z.ZodDiscriminatedUnion<[z.ZodObject<{
    kind: z.ZodLiteral<"url">;
    url: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>, z.ZodObject<{
    kind: z.ZodLiteral<"gcs">;
    bucket: z.ZodString;
    object: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>], "kind">>>;
  aspectRatio: z.ZodOptional<z.ZodEnum<{
    "1:1": "1:1";
    "4:3": "4:3";
    "3:4": "3:4";
    "16:9": "16:9";
    "9:16": "9:16";
    "21:9": "21:9";
  }>>;
  count: z.ZodDefault<z.ZodNumber>;
}, z.core.$strip>;
declare const videoGenerationInput: z.ZodObject<{
  model: z.ZodString;
  prompt: z.ZodString;
  negativePrompt: z.ZodOptional<z.ZodString>;
  seed: z.ZodOptional<z.ZodNumber>;
  startFrame: z.ZodOptional<z.ZodDiscriminatedUnion<[z.ZodObject<{
    kind: z.ZodLiteral<"url">;
    url: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>, z.ZodObject<{
    kind: z.ZodLiteral<"gcs">;
    bucket: z.ZodString;
    object: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>], "kind">>;
  endFrame: z.ZodOptional<z.ZodDiscriminatedUnion<[z.ZodObject<{
    kind: z.ZodLiteral<"url">;
    url: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>, z.ZodObject<{
    kind: z.ZodLiteral<"gcs">;
    bucket: z.ZodString;
    object: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>], "kind">>;
  aspectRatio: z.ZodOptional<z.ZodEnum<{
    "1:1": "1:1";
    "4:3": "4:3";
    "3:4": "3:4";
    "16:9": "16:9";
    "9:16": "9:16";
  }>>;
  duration: z.ZodOptional<z.ZodNumber>;
  generateAudio: z.ZodOptional<z.ZodBoolean>;
}, z.core.$strip>;
declare const soundGenerationInput: z.ZodObject<{
  model: z.ZodString;
  prompt: z.ZodString;
  negativePrompt: z.ZodOptional<z.ZodString>;
  seed: z.ZodOptional<z.ZodNumber>;
  duration: z.ZodOptional<z.ZodNumber>;
}, z.core.$strip>;
declare const transcriptionInput: z.ZodObject<{
  audio: z.ZodDiscriminatedUnion<[z.ZodObject<{
    kind: z.ZodLiteral<"url">;
    url: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>, z.ZodObject<{
    kind: z.ZodLiteral<"gcs">;
    bucket: z.ZodString;
    object: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>], "kind">;
  languageCode: z.ZodOptional<z.ZodString>;
}, z.core.$strip>;
declare const transcriptionOutput: z.ZodArray<z.ZodObject<{
  text: z.ZodString;
  words: z.ZodArray<z.ZodObject<{
    text: z.ZodString;
    start: z.ZodNumber;
    end: z.ZodNumber;
  }, z.core.$strip>>;
}, z.core.$strip>>;
declare const getUploadUrlInput: z.ZodObject<{
  id: z.ZodString;
  contentType: z.ZodDefault<z.ZodString>;
  action: z.ZodDefault<z.ZodEnum<{
    write: "write";
    resumable: "resumable";
  }>>;
}, z.core.$strip>;
declare const analyzeInput: z.ZodObject<{
  media: z.ZodDiscriminatedUnion<[z.ZodObject<{
    kind: z.ZodLiteral<"url">;
    url: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>, z.ZodObject<{
    kind: z.ZodLiteral<"gcs">;
    bucket: z.ZodString;
    object: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>], "kind">;
  prompt: z.ZodOptional<z.ZodString>;
}, z.core.$strip>;
declare const removeBackgroundInput: z.ZodObject<{
  image: z.ZodDiscriminatedUnion<[z.ZodObject<{
    kind: z.ZodLiteral<"url">;
    url: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>, z.ZodObject<{
    kind: z.ZodLiteral<"gcs">;
    bucket: z.ZodString;
    object: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>], "kind">;
}, z.core.$strip>;
type Transcript = z.infer<typeof transcriptionOutput>;
type WordGroup = Transcript[number]['words'];
declare const upscaleImageInput: z.ZodObject<{
  image: z.ZodDiscriminatedUnion<[z.ZodObject<{
    kind: z.ZodLiteral<"url">;
    url: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>, z.ZodObject<{
    kind: z.ZodLiteral<"gcs">;
    bucket: z.ZodString;
    object: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>], "kind">;
}, z.core.$strip>;
declare const upscaleVideoInput: z.ZodObject<{
  video: z.ZodDiscriminatedUnion<[z.ZodObject<{
    kind: z.ZodLiteral<"url">;
    url: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>, z.ZodObject<{
    kind: z.ZodLiteral<"gcs">;
    bucket: z.ZodString;
    object: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>], "kind">;
}, z.core.$strip>;
declare const addAudioToVideoInput: z.ZodObject<{
  video: z.ZodDiscriminatedUnion<[z.ZodObject<{
    kind: z.ZodLiteral<"url">;
    url: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>, z.ZodObject<{
    kind: z.ZodLiteral<"gcs">;
    bucket: z.ZodString;
    object: z.ZodString;
    assetId: z.ZodOptional<z.ZodString>;
  }, z.core.$strip>], "kind">;
}, z.core.$strip>;
declare const planSchema: z.ZodEnum<{
  free: "free";
  plus: "plus";
}>;
type Plan = z.infer<typeof planSchema>;
declare const subscriptionCreditsSchema: z.ZodEnum<{
  "2_500": "2_500";
  "5_000": "5_000";
  "15_000": "15_000";
  "30_000": "30_000";
  "45_000": "45_000";
}>;
type SubscriptionCredits = z.infer<typeof subscriptionCreditsSchema>;
declare const topupCreditsSchema: z.ZodEnum<{
  "5_000": "5_000";
  "1_000": "1_000";
  "2_000": "2_000";
  "10_000": "10_000";
  "25_000": "25_000";
}>;
type TopupCredits = z.infer<typeof topupCreditsSchema>;
declare const billingPeriodSchema: z.ZodEnum<{
  month: "month";
  year: "year";
}>;
type BillingPeriod = z.infer<typeof billingPeriodSchema>;
declare const stripeMetadataSchema: z.ZodObject<{
  type: z.ZodDefault<z.ZodEnum<{
    subscription: "subscription";
    topup: "topup";
  }>>;
  credits: z.ZodDefault<z.ZodString>;
  user_id: z.ZodString;
}, z.core.$strip>;
type StripeMetadata = z.infer<typeof stripeMetadataSchema>;
declare const ttsInput: z.ZodObject<{
  model: z.ZodString;
  prompt: z.ZodString;
  negativePrompt: z.ZodOptional<z.ZodString>;
  seed: z.ZodOptional<z.ZodNumber>;
  voice: z.ZodOptional<z.ZodString>;
  outputFormat: z.ZodDefault<z.ZodEnum<{
    mp3: "mp3";
    wav: "wav";
    ogg: "ogg";
    flac: "flac";
  }>>;
}, z.core.$strip>;
//#endregion
//#region src/types.d.ts
/** Any file the caller can pass in as a reference or edit target */
type FileRef = {
  kind: "url";
  url: string;
} | {
  kind: "gcs";
  bucket: string;
  object: string;
};
type GenerationResult = {
  url: string;
  mimeType: string; /** Width × height for image/video; duration (s) for audio */
  meta?: Record<string, unknown>;
};
/** Mirrors Stripe's invoice statuses without exposing Stripe types in the contract. */
type InvoiceStatus = "draft" | "open" | "paid" | "uncollectible" | "void";
/** Row shape of the Supabase `user_data` table. */
type UserData = {
  id: string;
  lifetime_credit_balance: number;
  monthly_credit_balance: number;
  monthly_credit_quota: number;
  access_level: number;
  stripe_customer_id: string | null;
  last_credit_reset_at: string | null;
  product_updates_enabled: boolean;
  marketing_announcements_enabled: boolean;
};
declare const ACCESS_LEVELS: {
  readonly free: 1;
  readonly plus: 2;
};
declare const FREE_CREDITS_QUOTA = 50;
//#endregion
export { ACCESS_LEVELS, type AppRouter, BillingPeriod, FREE_CREDITS_QUOTA, FileRef, GenerationResult, InvoiceStatus, Plan, StripeMetadata, SubscriptionCredits, TopupCredits, Transcript, UserData, WordGroup, addAudioToVideoInput, analyzeInput, baseGenerationInput, billingPeriodSchema, fileRef, getUploadUrlInput, imageGenerationInput, planSchema, removeBackgroundInput, soundGenerationInput, stripeMetadataSchema, subscriptionCreditsSchema, topupCreditsSchema, transcriptionInput, transcriptionOutput, ttsInput, upscaleImageInput, upscaleVideoInput, videoGenerationInput };