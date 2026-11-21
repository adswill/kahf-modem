/** Letters and digits, 3 to 10 characters, with at least one digit (amateur callsigns have one). */
export function normaliseCall(text: string): string {
  return text.trim().toUpperCase();
}

export function validCall(call: string): boolean {
  return /^[A-Z0-9]{3,10}$/.test(call) && /[0-9]/.test(call) && /[A-Z]/.test(call);
}

export function callProblem(text: string): string | null {
  const c = normaliseCall(text);
  if (c === "") return null;
  if (!/^[A-Z0-9/]*$/.test(c)) return "Use letters and numbers only.";
  if (c.length < 3) return "A callsign has at least 3 characters.";
  if (c.length > 10) return "That is too long for a callsign.";
  if (!/[0-9]/.test(c)) return "A callsign has at least one number, like KA1ABC.";
  return null;
}
