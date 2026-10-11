import { z } from "zod";
//#region src/schemas.ts
const fileRef = z.discriminatedUnion("kind", [z.object({
	kind: z.literal("url"),
	url: z.string().url(),
	assetId: z.string().optional()
}), z.object({
	kind: z.literal("gcs"),
	bucket: z.string(),
	object: z.string(),
	assetId: z.string().optional()
})]);
const baseGenerationInput = z.object({
	model: z.string(),
	prompt: z.string(),
	negativePrompt: z.string().optional(),
	seed: z.number().int().optional()
});
const imageGenerationInput = baseGenerationInput.extend({
	/** Reference images or images to edit */
	images: z.array(fileRef).optional(),
	aspectRatio: z.enum([
		"1:1",
		"4:3",
		"3:4",
		"16:9",
		"9:16",
		"21:9"
	]).optional(),
	count: z.number().int().min(1).max(10).default(1)
});
const videoGenerationInput = baseGenerationInput.extend({
	startFrame: fileRef.optional(),
	endFrame: fileRef.optional(),
	aspectRatio: z.enum([
		"1:1",
		"4:3",
		"3:4",
		"16:9",
		"9:16"
	]).optional(),
	duration: z.number().int().min(1).max(60).optional(),
	generateAudio: z.boolean().optional()
});
const soundGenerationInput = baseGenerationInput.extend({ 
/** Requested duration  */
duration: z.number().positive().optional() });
const transcriptionInput = z.object({
	audio: fileRef,
	languageCode: z.string().optional()
});
const transcriptionOutput = z.array(z.object({
	text: z.string(),
	words: z.array(z.object({
		text: z.string(),
		start: z.number().describe("Start time in seconds"),
		end: z.number().describe("End time in seconds")
	}))
}));
const getUploadUrlInput = z.object({
	id: z.string().min(1),
	contentType: z.string().default("application/octet-stream"),
	action: z.enum(["write", "resumable"]).default("write")
});
const analyzeInput = z.object({
	media: fileRef,
	prompt: z.string().optional()
});
const removeBackgroundInput = z.object({ image: fileRef });
const upscaleImageInput = z.object({ image: fileRef });
const upscaleVideoInput = z.object({ video: fileRef });
const addAudioToVideoInput = z.object({ video: fileRef });
const planSchema = z.enum(["free", "plus"]);
const subscriptionCreditsSchema = z.enum([
	"2_500",
	"5_000",
	"15_000",
	"30_000",
	"45_000"
]);
const topupCreditsSchema = z.enum([
	"1_000",
	"2_000",
	"5_000",
	"10_000",
	"25_000"
]);
const billingPeriodSchema = z.enum(["month", "year"]);
const stripeMetadataSchema = z.object({
	type: z.enum(["subscription", "topup"]).default("subscription"),
	credits: z.string().default("0"),
	user_id: z.string()
});
const ttsInput = baseGenerationInput.extend({
	/**
	* Voice identifier. Interpretation is model-dependent:
	*   ElevenLabs → voice_id  (e.g. "EXAVITQu4vr4xnSDxMaL")
	*   OpenAI TTS → "alloy" | "echo" | "fable" | "onyx" | "nova" | "shimmer"
	*   GCP TTS   → BCP-47 + SSML gender, e.g. "en-US-Neural2-A"
	*/
	voice: z.string().optional(),
	outputFormat: z.enum([
		"mp3",
		"wav",
		"ogg",
		"flac"
	]).default("mp3")
});
//#endregion
//#region src/types.ts
const ACCESS_LEVELS = {
	free: 1,
	plus: 2
};
const FREE_CREDITS_QUOTA = 50;
//#endregion
export { ACCESS_LEVELS, FREE_CREDITS_QUOTA, addAudioToVideoInput, analyzeInput, baseGenerationInput, billingPeriodSchema, fileRef, getUploadUrlInput, imageGenerationInput, planSchema, removeBackgroundInput, soundGenerationInput, stripeMetadataSchema, subscriptionCreditsSchema, topupCreditsSchema, transcriptionInput, transcriptionOutput, ttsInput, upscaleImageInput, upscaleVideoInput, videoGenerationInput };
