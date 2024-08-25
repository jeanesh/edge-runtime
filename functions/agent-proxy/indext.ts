import { serve } from "https://deno.land/std@0.177.0/http/server.ts";

const FLOWISE_API_URL = Deno.env.get("FLOWISE_API_URL");
const FLOWISE_API_KEY = Deno.env.get("FLOWISE_API_KEY");

if (!FLOWISE_API_URL || !FLOWISE_API_KEY) {
  throw new Error("FLOWISE_API_URL and FLOWISE_API_KEY must be set in environment variables");
}

interface FlowiseRequest {
  question: string;
  userId: string;
  chatflowId: string;
}

serve(async (req: Request): Promise<Response> => {
  // CORS headers
  const headers = new Headers({
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Methods": "POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type, Authorization",
  });

  // Handle preflight requests
  if (req.method === "OPTIONS") {
    return new Response(null, { headers });
  }

  if (req.method !== "POST") {
    return new Response("Method Not Allowed", { status: 405, headers });
  }

  try {
    const { question, userId, chatflowId } = await req.json() as FlowiseRequest;

    if (!question || !userId || !chatflowId) {
      return new Response(JSON.stringify({ error: "Missing required parameters" }), {
        status: 400,
        headers: { ...headers, "Content-Type": "application/json" }
      });
    }

    const flowiseReq = new Request(`${FLOWISE_API_URL}/api/v1/prediction/${chatflowId}`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "Authorization": `Bearer ${FLOWISE_API_KEY}`
      },
      body: JSON.stringify({
        question,
        overrideConfig: { userId }
      })
    });

    const flowiseResponse = await fetch(flowiseReq);

    if (!flowiseResponse.ok) {
      throw new Error(`Flowise API responded with status: ${flowiseResponse.status}`);
    }

    // Stream the response
    const { readable, writable } = new TransformStream();
    flowiseResponse.body?.pipeTo(writable);

    return new Response(readable, {
      headers: {
        ...headers,
        "Content-Type": "text/event-stream",
        "Cache-Control": "no-cache",
        "Connection": "keep-alive"
      }
    });

  } catch (error) {
    console.error("Error in Flowise proxy:", error);
    return new Response(JSON.stringify({ error: "Internal Server Error" }), {
      status: 500,
      headers: { ...headers, "Content-Type": "application/json" }
    });
  }
});
