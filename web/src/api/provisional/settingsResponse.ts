export async function settingsResponse<T>(response: Response): Promise<T | null> {
  if (
    [404, 405].includes(response.status) ||
    (response.ok && response.headers.get("Content-Type")?.includes("text/html"))
  )
    return null;
  if (!response.ok) {
    const body: { message?: string } | null = await response.json().catch(() => null);
    throw new Error(body?.message ?? "Could not read settings. Try again.");
  }
  return (await response.json()) as T;
}
